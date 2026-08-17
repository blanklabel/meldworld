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
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// One saved party composition (PT-2): classes by slot, plus the gear that was worn.
#[derive(Debug, Clone, Default)]
pub struct Loadout {
    pub name: String,
    pub classes: Vec<String>,
    /// `(hero_slot, gear_id)` pairs. Ids only — the item's stats are read live from
    /// `gear` at load time, so a loadout can never restore an item's old numbers.
    pub gear: Vec<(i32, Uuid)>,
}

/// Split a stored `classes` column back into slot order, dropping empties so a
/// trailing comma cannot conjure a phantom slot.
/// The stored column word for a tier. `blue`/`red` are the legacy chest colours the
/// column has always used; `standard` is the third tier joining them.
fn insurance_word(i: meld_proto::Insurance) -> &'static str {
    match i {
        meld_proto::Insurance::Insured => "blue",
        meld_proto::Insurance::Ephemeral => "red",
        meld_proto::Insurance::Standard => "standard",
    }
}

fn split_classes(joined: &str) -> Vec<String> {
    joined.split(',').filter(|c| !c.is_empty()).map(str::to_string).collect()
}

/// Parse the stored `gear` column back into `(hero_slot, gear_id)` pairs, dropping
/// anything unparseable rather than failing the read — a malformed pair costs one
/// item at load time, not the whole loadout.
fn split_gear(joined: &str) -> Vec<(i32, Uuid)> {
    joined
        .split(',')
        .filter(|p| !p.is_empty())
        .filter_map(|p| {
            let (slot, id) = p.split_once(':')?;
            Some((slot.parse().ok()?, Uuid::parse_str(id).ok()?))
        })
        .collect()
}

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
        // GR-7: the hero's class, persisted per slot. A hero is a character, not a
        // slot the next dive redefines — and equip-time legality (GR-5) needs a
        // class to check against while the player is in town, outside any run.
        sqlx::query("ALTER TABLE heroes ADD COLUMN IF NOT EXISTS class_key TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        // GR-5 equipment identity: an item's weapon family (sword/staff/globe/…)
        // and armor weight band. Nullable-as-empty: a row with neither descriptor
        // is unrestricted, so nothing already in a Vault becomes unwearable.
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS family TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS armor_weight TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        // AD-1 affixes, as the JSON array `meld_proto::affixes` serializes. `[]`
        // (or unreadable content) is simply no affixes, never a broken item.
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS affixes TEXT NOT NULL DEFAULT '[]'")
            .execute(pool)
            .await?;
        // AD-1 chase tiers: which authored unique this is, and which set it belongs
        // to. Empty for ordinary loot.
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS unique_key TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        sqlx::query("ALTER TABLE gear ADD COLUMN IF NOT EXISTS set_key TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        // The high-water mark per CLASS: the deepest level any hero of that class has
        // ever reached. XP itself is dive-scoped and never persists — this is the
        // record of what was achieved, which is what the unlock rules and the roster
        // screen read.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS class_bests (
                player_id UUID NOT NULL REFERENCES players(player_id),
                class_key TEXT NOT NULL,
                best_level INTEGER NOT NULL,
                PRIMARY KEY (player_id, class_key)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Account-permanent unlocks (roadmap CL-1): the party slots and classes a
        // player has EARNED. Additive-only and never deleted — an unlock is a
        // promise, so there is deliberately no revoke path.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS party_loadouts (
                player_id  UUID NOT NULL REFERENCES players(player_id),
                name       TEXT NOT NULL,
                -- One class key per slot, in slot order, comma-joined. A loadout is a
                -- COMPOSITION, not a set of hero ids: hero slots are positional, so
                -- "slot 2 is a Resonant" is the whole content and a join table would
                -- carry nothing else.
                classes    TEXT NOT NULL,
                -- The gear that was equipped when this was saved, as
                -- `hero_slot:gear_id` pairs, comma-joined. Gear ids only: what an item
                -- IS lives in `gear`, and duplicating its stats here would let a stale
                -- loadout resurrect an item's old numbers.
                gear       TEXT NOT NULL DEFAULT '',
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (player_id, name)
            )
            "#,
        )
        .execute(pool)
        .await?;
        sqlx::query("ALTER TABLE party_loadouts ADD COLUMN IF NOT EXISTS gear TEXT NOT NULL DEFAULT ''")
            .execute(pool)
            .await?;
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS unlocks (
                player_id UUID NOT NULL REFERENCES players(player_id),
                unlock_key TEXT NOT NULL,
                unlocked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                PRIMARY KEY (player_id, unlock_key)
            )
            "#,
        )
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
        // Additive: the board records HOW a run got deep, not only how far. Columns added
        // after the table shipped, so `IF NOT EXISTS` and a default keep old rows readable
        // (an existing posting simply reports nothing about its route).
        for col in [
            "at_level INTEGER NOT NULL DEFAULT 0",
            "fights INTEGER NOT NULL DEFAULT 0",
            "flees INTEGER NOT NULL DEFAULT 0",
            // The END FIGHT's mark, and how long it took. NULLable: most postings have not
            // felled it, and 0 would read as "cleared instantly".
            "star TEXT",
            "clear_ms BIGINT",
        ] {
            sqlx::query(&format!("ALTER TABLE vanguard ADD COLUMN IF NOT EXISTS {col}"))
                .execute(pool)
                .await?;
        }
        // Board reads are `ORDER BY max_distance DESC, achieved_at ASC` within one
        // season — index that exact shape so the live board stays a cheap query.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_vanguard_rank ON vanguard(season, max_distance DESC, achieved_at ASC)",
        )
        .execute(pool)
        .await?;
        // The Hunt Board (roadmap AD-4): one row per hunt a player has made progress
        // on. `progress` is capped at the hunt's target by every writer, so "complete"
        // is `progress >= target` read against the registry rather than a second column
        // that could disagree with it. `claimed_at` is what makes a payout once-only.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS hunts (
                player_id  UUID NOT NULL REFERENCES players(player_id),
                hunt_key   TEXT NOT NULL,
                progress   INTEGER NOT NULL DEFAULT 0,
                claimed_at TIMESTAMPTZ,
                PRIMARY KEY (player_id, hunt_key)
            )
            "#,
        )
        .execute(pool)
        .await?;
        // Bounties (roadmap AD-4): the Den's generated contracts, one row per rolled
        // mark. The rolled numbers live in `spec` as JSON rather than as columns — they
        // are drawn once against `[bounty]` and then owned by the contract, so a retune
        // changes the next roll instead of rewriting one a player is already working.
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS bounties (
                bounty_id  UUID PRIMARY KEY,
                player_id  UUID NOT NULL REFERENCES players(player_id),
                spec       TEXT NOT NULL,
                state      TEXT NOT NULL,
                expires_at TIMESTAMPTZ NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )
            "#,
        )
        .execute(pool)
        .await?;
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_bounties_player ON bounties(player_id, state)",
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
    /// The deepest distance this player has EVER reached, across every season (PG-2).
    ///
    /// All-time on purpose: the live-season read is what the board shows, but a season
    /// rollover must not revoke a departure hub you demonstrably stood on. `0` when there
    /// is no record at all, which is a brand-new account — and the Center Hub clears that.
    pub async fn deepest_distance_ever(&self, player_id: Uuid) -> Result<i32, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let row: Option<(i32,)> = sqlx::query_as(
                    "SELECT COALESCE(MAX(max_distance), 0) FROM vanguard WHERE player_id = $1",
                )
                .bind(player_id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| r.0).unwrap_or(0))
            }
            Backend::Mem(m) => {
                let g = m.lock().unwrap();
                Ok(g.vanguard
                    .iter()
                    .filter(|((_, pid), _)| *pid == player_id)
                    .map(|(_, v)| v.distance)
                    .max()
                    .unwrap_or(0))
            }
        }
    }

    /// Post a new deepest tile, with HOW the run got there (level / fights / flees).
    ///
    /// The route travels with the distance because it is the interesting half: 500 fights
    /// and 0 fights are the same tile and completely different runs. The whole row moves
    /// together on a deeper posting — a shallower one is still a true no-op, so a route is
    /// never stitched from two different runs.
    pub async fn record_vanguard_distance(
        &self,
        player_id: Uuid,
        season: i32,
        distance: i32,
        at_level: i32,
        fights: i32,
        flees: i32,
    ) -> Result<bool, DbError> {
        if distance <= 0 {
            return Ok(false);
        }
        match &self.backend {
            Backend::Pg(pool) => {
                // The `WHERE` makes a shallower post a true no-op: neither the
                // distance nor the timestamp moves.
                let res = sqlx::query(
                    "INSERT INTO vanguard (season, player_id, max_distance, at_level, fights, flees)
                     VALUES ($1, $2, $3, $4, $5, $6)
                     ON CONFLICT (season, player_id) DO UPDATE
                       SET max_distance = $3, at_level = $4, fights = $5, flees = $6,
                           achieved_at = now()
                       WHERE vanguard.max_distance < $3",
                )
                .bind(season)
                .bind(player_id)
                .bind(distance)
                .bind(at_level)
                .bind(fights)
                .bind(flees)
                .execute(pool)
                .await?;
                Ok(res.rows_affected() > 0)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let e = m.vanguard.entry((season, player_id)).or_insert(MemVanguard {
                    distance: 0,
                    at: Utc::now(),
                    at_level: 0,
                    fights: 0,
                    flees: 0,
                    star: false,
                    clear_ms: None,
                });
                if e.distance < distance {
                    // A deeper posting keeps a star already earned this season — the star is
                    // for felling the end fight, not for the tile it happened on.
                    *e = MemVanguard {
                        distance,
                        at: Utc::now(),
                        at_level,
                        fights,
                        flees,
                        star: e.star,
                        clear_ms: e.clear_ms,
                    };
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Star a posting: the END FIGHT is down, and the board says so and how long it took.
    ///
    /// A **wood** star is the first rung on purpose — beating three of the world's bosses
    /// together is the current top of the game, and the material leaves room above it for
    /// whatever the real end (EW-4's Ometus) turns out to be worth.
    pub async fn record_world_end(
        &self,
        player_id: Uuid,
        season: i32,
        stamp: &meld_proto::realtime::run::VanguardStamp,
        clear_ms: i64,
    ) -> Result<bool, DbError> {
        // Post the depth first so a star always sits on a real row, then star it. Keeping
        // the star's own `WHERE` off the distance means a slower clear never overwrites a
        // faster one just because it went one tile deeper.
        let _ = self
            .record_vanguard_distance(
                player_id,
                season,
                stamp.distance.max(1),
                stamp.level,
                stamp.fights,
                stamp.flees,
            )
            .await?;
        match &self.backend {
            Backend::Pg(pool) => {
                let res = sqlx::query(
                    "UPDATE vanguard SET star = 'wood', clear_ms = $3
                      WHERE season = $1 AND player_id = $2
                        AND (clear_ms IS NULL OR clear_ms > $3)",
                )
                .bind(season)
                .bind(player_id)
                .bind(clear_ms)
                .execute(pool)
                .await?;
                Ok(res.rows_affected() > 0)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                if let Some(v) = m.vanguard.get_mut(&(season, player_id)) {
                    if v.clear_ms.is_none_or(|c| c > clear_ms) {
                        v.star = true;
                        v.clear_ms = Some(clear_ms);
                        return Ok(true);
                    }
                }
                Ok(false)
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
                    "SELECT v.player_id, p.username, v.max_distance, v.achieved_at,
                            v.at_level, v.fights, v.flees, v.star, v.clear_ms
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
                        at_level: r.get("at_level"),
                        fights: r.get("fights"),
                        flees: r.get("flees"),
                        star: r.get("star"),
                        clear_ms: r.get("clear_ms"),
                    })
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<VanguardRow> = m
                    .vanguard
                    .iter()
                    .filter(|((s, _), _)| *s == season)
                    .filter_map(|((_, pid), v)| {
                        m.players.get(pid).map(|p| VanguardRow {
                            player_id: *pid,
                            username: p.username.clone(),
                            max_distance: v.distance,
                            achieved_at: v.at,
                            at_level: v.at_level,
                            fights: v.fights,
                            flees: v.flees,
                            star: v.star.then(|| "wood".to_string()),
                            clear_ms: v.clear_ms,
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

    /// Every hunt this account has touched (roadmap AD-4). A hunt with no row has
    /// never been progressed; the board fills the gaps from the registry.
    pub async fn get_hunts(&self, player_id: Uuid) -> Result<Vec<HuntRow>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT hunt_key, progress, claimed_at FROM hunts WHERE player_id = $1",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .iter()
                    .map(|r| HuntRow {
                        hunt_key: r.get("hunt_key"),
                        progress: r.get("progress"),
                        claimed: r.get::<Option<DateTime<Utc>>, _>("claimed_at").is_some(),
                    })
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                Ok(m.hunts
                    .iter()
                    .filter(|((pid, _), _)| *pid == player_id)
                    .map(|((_, key), (progress, claimed))| HuntRow {
                        hunt_key: key.clone(),
                        progress: *progress,
                        claimed: *claimed,
                    })
                    .collect())
            }
        }
    }

    /// Add `delta` to a hunt's progress, capped at `target`.
    ///
    /// `completed` is true only on the credit that crosses the target, so the loop can
    /// announce a finished hunt exactly once however many kills land in the same tick.
    /// A claimed hunt is frozen: re-earning it would let one payout be taken twice.
    pub async fn credit_hunt(
        &self,
        player_id: Uuid,
        hunt_key: &str,
        delta: i32,
        target: i32,
    ) -> Result<HuntCredit, DbError> {
        if delta <= 0 || target <= 0 {
            return Ok(HuntCredit { progress: 0, completed: false });
        }
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let before: Option<(i32, Option<DateTime<Utc>>)> = sqlx::query(
                    "SELECT progress, claimed_at FROM hunts
                      WHERE player_id = $1 AND hunt_key = $2 FOR UPDATE",
                )
                .bind(player_id)
                .bind(hunt_key)
                .fetch_optional(&mut *tx)
                .await?
                .map(|r| (r.get("progress"), r.get("claimed_at")));
                let (was, claimed) = before.unwrap_or((0, None));
                if claimed.is_some() || was >= target {
                    tx.rollback().await?;
                    return Ok(HuntCredit { progress: was.min(target), completed: false });
                }
                let now = (was + delta).min(target);
                sqlx::query(
                    "INSERT INTO hunts (player_id, hunt_key, progress) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, hunt_key) DO UPDATE SET progress = $3",
                )
                .bind(player_id)
                .bind(hunt_key)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(HuntCredit { progress: now, completed: now >= target })
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let e = m.hunts.entry((player_id, hunt_key.to_string())).or_insert((0, false));
                if e.1 || e.0 >= target {
                    return Ok(HuntCredit { progress: e.0.min(target), completed: false });
                }
                e.0 = (e.0 + delta).min(target);
                Ok(HuntCredit { progress: e.0, completed: e.0 >= target })
            }
        }
    }

    /// Pay a completed hunt out: mark it claimed and credit the reward, atomically.
    ///
    /// The claim stamp and the payout are one transaction, so a board cannot pay twice
    /// under concurrent presses — the second one reads a stamped row and refuses.
    pub async fn claim_hunt(
        &self,
        player_id: Uuid,
        hunt_key: &str,
        target: i32,
        chits: i64,
        material: Option<(&str, i32)>,
        gear: Option<&LootedGear>,
    ) -> Result<HuntClaim, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let row: Option<(i32, Option<DateTime<Utc>>)> = sqlx::query(
                    "SELECT progress, claimed_at FROM hunts
                      WHERE player_id = $1 AND hunt_key = $2 FOR UPDATE",
                )
                .bind(player_id)
                .bind(hunt_key)
                .fetch_optional(&mut *tx)
                .await?
                .map(|r| (r.get("progress"), r.get("claimed_at")));
                let (progress, claimed) = row.unwrap_or((0, None));
                if claimed.is_some() {
                    tx.rollback().await?;
                    return Ok(HuntClaim::AlreadyClaimed);
                }
                if progress < target {
                    tx.rollback().await?;
                    return Ok(HuntClaim::NotEarned { progress });
                }
                sqlx::query(
                    "INSERT INTO hunts (player_id, hunt_key, progress, claimed_at)
                     VALUES ($1, $2, $3, now())
                     ON CONFLICT (player_id, hunt_key) DO UPDATE SET claimed_at = now()",
                )
                .bind(player_id)
                .bind(hunt_key)
                .bind(progress)
                .execute(&mut *tx)
                .await?;
                let after: i64 = sqlx::query(
                    "INSERT INTO vaults (player_id, chits) VALUES ($1, $2)
                     ON CONFLICT (player_id) DO UPDATE SET chits = vaults.chits + $2
                     RETURNING chits",
                )
                .bind(player_id)
                .bind(chits)
                .fetch_one(&mut *tx)
                .await?
                .get("chits");
                if let Some((kind, qty)) = material.filter(|(_, q)| *q > 0) {
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
                if let Some(g) = gear {
                    insert_gear_row(&mut tx, player_id, g).await?;
                }
                tx.commit().await?;
                Ok(HuntClaim::Paid { chits: after })
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let (progress, claimed) = m
                    .hunts
                    .get(&(player_id, hunt_key.to_string()))
                    .copied()
                    .unwrap_or((0, false));
                if claimed {
                    return Ok(HuntClaim::AlreadyClaimed);
                }
                if progress < target {
                    return Ok(HuntClaim::NotEarned { progress });
                }
                m.hunts.insert((player_id, hunt_key.to_string()), (progress, true));
                let after = {
                    let c = m.chits.entry(player_id).or_insert(0);
                    *c += chits;
                    *c
                };
                if let Some((kind, qty)) = material.filter(|(_, q)| *q > 0) {
                    *m.vault_items.entry((player_id, kind.to_string())).or_insert(0) += qty;
                }
                if let Some(g) = gear {
                    m.gear.entry(g.gear_id).or_insert_with(|| mem_gear_row(player_id, g));
                }
                Ok(HuntClaim::Paid { chits: after })
            }
        }
    }

    /// Every bounty contract this account has ever been offered, newest first (AD-4).
    pub async fn list_bounties(&self, player_id: Uuid) -> Result<Vec<BountyRow>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT bounty_id, spec, state, expires_at, created_at FROM bounties
                      WHERE player_id = $1 ORDER BY created_at DESC, bounty_id DESC",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .iter()
                    .map(|r| BountyRow {
                        bounty_id: r.get("bounty_id"),
                        spec: r.get("spec"),
                        state: r.get("state"),
                        expires_at: r.get("expires_at"),
                        created_at: r.get("created_at"),
                    })
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<BountyRow> = m
                    .bounties
                    .values()
                    .filter(|b| b.player_id == player_id)
                    .map(|b| BountyRow {
                        bounty_id: b.bounty_id,
                        spec: b.spec.clone(),
                        state: b.state.clone(),
                        expires_at: b.expires_at,
                        created_at: b.created_at,
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    b.created_at.cmp(&a.created_at).then(b.bounty_id.cmp(&a.bounty_id))
                });
                Ok(rows)
            }
        }
    }

    /// Post a freshly rolled contract.
    pub async fn insert_bounty(
        &self,
        player_id: Uuid,
        bounty_id: Uuid,
        spec: &str,
        expires_at: DateTime<Utc>,
    ) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "INSERT INTO bounties (bounty_id, player_id, spec, state, expires_at)
                     VALUES ($1, $2, $3, 'active', $4)
                     ON CONFLICT (bounty_id) DO NOTHING",
                )
                .bind(bounty_id)
                .bind(player_id)
                .bind(spec)
                .bind(expires_at)
                .execute(pool)
                .await?;
                Ok(())
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                m.bounties.entry(bounty_id).or_insert(MemBounty {
                    bounty_id,
                    player_id,
                    spec: spec.to_string(),
                    state: "active".to_string(),
                    expires_at,
                    created_at: Utc::now(),
                });
                Ok(())
            }
        }
    }

    /// Withdraw every standing contract whose window has closed. Returns how many.
    ///
    /// Only an `active` row expires: a mark already felled is owed its reward however
    /// long the walk home takes.
    pub async fn expire_bounties(
        &self,
        player_id: Uuid,
        now: DateTime<Utc>,
    ) -> Result<u64, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let res = sqlx::query(
                    "UPDATE bounties SET state = 'expired'
                      WHERE player_id = $1 AND state = 'active' AND expires_at <= $2",
                )
                .bind(player_id)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(res.rows_affected())
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let mut n = 0;
                for b in m.bounties.values_mut() {
                    if b.player_id == player_id && b.state == "active" && b.expires_at <= now {
                        b.state = "expired".to_string();
                        n += 1;
                    }
                }
                Ok(n)
            }
        }
    }

    /// Mark a contract's mark as felled. `true` when this call is the one that did it.
    pub async fn complete_bounty(
        &self,
        player_id: Uuid,
        bounty_id: Uuid,
    ) -> Result<bool, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let res = sqlx::query(
                    "UPDATE bounties SET state = 'completed'
                      WHERE bounty_id = $1 AND player_id = $2 AND state = 'active'",
                )
                .bind(bounty_id)
                .bind(player_id)
                .execute(pool)
                .await?;
                Ok(res.rows_affected() > 0)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                match m.bounties.get_mut(&bounty_id) {
                    Some(b) if b.player_id == player_id && b.state == "active" => {
                        b.state = "completed".to_string();
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
        }
    }

    /// Pay a finished contract out and bank its hunter XP, atomically (AD-4).
    ///
    /// The hunter rank rides the `hunting` Meld skill, so the same ladder every other
    /// profession uses carries the Den's — and the XP lands in the same transaction as
    /// the payout, because a rank that moved without paying is a rank nobody earned.
    pub async fn claim_bounty(
        &self,
        player_id: Uuid,
        bounty_id: Uuid,
        chits: i64,
        material: Option<(&str, i32)>,
        gear: Option<&LootedGear>,
        rank_xp: i64,
    ) -> Result<BountyClaim, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let state: Option<String> = sqlx::query(
                    "SELECT state FROM bounties WHERE bounty_id = $1 AND player_id = $2 FOR UPDATE",
                )
                .bind(bounty_id)
                .bind(player_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|r| r.get("state"));
                match state.as_deref() {
                    None => {
                        tx.rollback().await?;
                        return Ok(BountyClaim::Missing);
                    }
                    Some("claimed") => {
                        tx.rollback().await?;
                        return Ok(BountyClaim::AlreadyClaimed);
                    }
                    Some("completed") => {}
                    Some(_) => {
                        tx.rollback().await?;
                        return Ok(BountyClaim::NotCompleted);
                    }
                }
                sqlx::query("UPDATE bounties SET state = 'claimed' WHERE bounty_id = $1")
                    .bind(bounty_id)
                    .execute(&mut *tx)
                    .await?;
                let after: i64 = sqlx::query(
                    "INSERT INTO vaults (player_id, chits) VALUES ($1, $2)
                     ON CONFLICT (player_id) DO UPDATE SET chits = vaults.chits + $2
                     RETURNING chits",
                )
                .bind(player_id)
                .bind(chits)
                .fetch_one(&mut *tx)
                .await?
                .get("chits");
                if let Some((kind, qty)) = material.filter(|(_, q)| *q > 0) {
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
                if let Some(g) = gear {
                    insert_gear_row(&mut tx, player_id, g).await?;
                }
                if rank_xp > 0 {
                    sqlx::query(
                        "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1, 'hunting', $2)
                         ON CONFLICT (player_id, skill_kind) DO UPDATE SET xp = meld_skills.xp + $2",
                    )
                    .bind(player_id)
                    .bind(rank_xp)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(BountyClaim::Paid { chits: after })
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                match m.bounties.get(&bounty_id).map(|b| (b.player_id, b.state.clone())) {
                    None => return Ok(BountyClaim::Missing),
                    Some((owner, _)) if owner != player_id => return Ok(BountyClaim::Missing),
                    Some((_, s)) if s == "claimed" => return Ok(BountyClaim::AlreadyClaimed),
                    Some((_, s)) if s != "completed" => return Ok(BountyClaim::NotCompleted),
                    Some(_) => {}
                }
                if let Some(b) = m.bounties.get_mut(&bounty_id) {
                    b.state = "claimed".to_string();
                }
                let after = {
                    let c = m.chits.entry(player_id).or_insert(0);
                    *c += chits;
                    *c
                };
                if let Some((kind, qty)) = material.filter(|(_, q)| *q > 0) {
                    *m.vault_items.entry((player_id, kind.to_string())).or_insert(0) += qty;
                }
                if let Some(g) = gear {
                    m.gear.entry(g.gear_id).or_insert_with(|| mem_gear_row(player_id, g));
                }
                if rank_xp > 0 {
                    *m.skills.entry((player_id, "hunting".to_string())).or_insert(0) += rank_xp;
                }
                Ok(BountyClaim::Paid { chits: after })
            }
        }
    }

    /// The account's saved party loadouts, newest-touched first, as
    /// `(name, class keys in slot order)`.
    pub async fn list_loadouts(&self, player_id: Uuid) -> Result<Vec<Loadout>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT name, classes, gear FROM party_loadouts
                      WHERE player_id = $1 ORDER BY updated_at DESC, name ASC",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                Ok(rows
                    .iter()
                    .map(|r| Loadout {
                        name: r.get("name"),
                        classes: split_classes(&r.get::<String, _>("classes")),
                        gear: split_gear(&r.get::<String, _>("gear")),
                    })
                    .collect())
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut out: Vec<Loadout> = m
                    .loadouts
                    .iter()
                    .filter(|((p, _), _)| *p == player_id)
                    .map(|((_, n), l)| Loadout { name: n.clone(), ..l.clone() })
                    .collect();
                // No timestamps in the mem store, so name order — deterministic, which
                // is what its callers (tests, the demo binary) actually need.
                out.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(out)
            }
        }
    }

    /// Save (or overwrite) a named loadout. Upsert on `(player_id, name)` so saving
    /// over a name is how you update one — there is no separate rename.
    pub async fn save_loadout(
        &self,
        player_id: Uuid,
        name: &str,
        classes: &[String],
        gear: &[(i32, Uuid)],
    ) -> Result<(), DbError> {
        let joined = classes.join(",");
        let gear_s = gear
            .iter()
            .map(|(slot, id)| format!("{slot}:{id}"))
            .collect::<Vec<_>>()
            .join(",");
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "INSERT INTO party_loadouts (player_id, name, classes, gear)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (player_id, name)
                       DO UPDATE SET classes = $3, gear = $4, updated_at = now()",
                )
                .bind(player_id)
                .bind(name)
                .bind(&joined)
                .bind(&gear_s)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                m.lock().unwrap().loadouts.insert(
                    (player_id, name.to_string()),
                    Loadout {
                        name: name.to_string(),
                        classes: classes.to_vec(),
                        gear: gear.to_vec(),
                    },
                );
            }
        }
        Ok(())
    }

    /// Forget a named loadout. Deleting one that is not there is not an error — the
    /// caller wanted it gone and it is gone.
    pub async fn delete_loadout(&self, player_id: Uuid, name: &str) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query("DELETE FROM party_loadouts WHERE player_id = $1 AND name = $2")
                    .bind(player_id)
                    .bind(name)
                    .execute(pool)
                    .await?;
            }
            Backend::Mem(m) => {
                m.lock().unwrap().loadouts.remove(&(player_id, name.to_string()));
            }
        }
        Ok(())
    }

    /// One player's placement for a season: their row and their rank across the WHOLE
    /// season, not just the board's first page. `None` if they never posted.
    ///
    /// Ranking off a `LIMIT`ed board silently reports "unranked" for everyone below the
    /// cut, which is precisely the player who needs to be told where they stand.
    pub async fn vanguard_placement(
        &self,
        season: i32,
        player_id: Uuid,
    ) -> Result<Option<(VanguardRow, i64)>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                // Same ordering as `vanguard_board`, so a player's rank here and their
                // position on the board agree.
                let row = sqlx::query(
                    "SELECT v.player_id, p.username, v.max_distance, v.achieved_at,
                            (SELECT count(*) + 1 FROM vanguard w
                              WHERE w.season = v.season
                                AND (-w.max_distance, w.achieved_at, w.player_id)
                                  < (-v.max_distance, v.achieved_at, v.player_id)) AS rank
                       FROM vanguard v JOIN players p USING (player_id)
                      WHERE v.season = $1 AND v.player_id = $2",
                )
                .bind(season)
                .bind(player_id)
                .fetch_optional(pool)
                .await?;
                Ok(row.map(|r| {
                    (
                        VanguardRow {
                            player_id: r.get("player_id"),
                            username: r.get("username"),
                            max_distance: r.get("max_distance"),
                            achieved_at: r.get::<DateTime<Utc>, _>("achieved_at"),
                            at_level: r.get("at_level"),
                            fights: r.get("fights"),
                            flees: r.get("flees"),
                            star: r.get("star"),
                            clear_ms: r.get("clear_ms"),
                        },
                        r.get::<i64, _>("rank"),
                    )
                }))
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<VanguardRow> = m
                    .vanguard
                    .iter()
                    .filter(|((s, _), _)| *s == season)
                    .filter_map(|((_, pid), v)| {
                        m.players.get(pid).map(|p| VanguardRow {
                            player_id: *pid,
                            username: p.username.clone(),
                            max_distance: v.distance,
                            achieved_at: v.at,
                            at_level: v.at_level,
                            fights: v.fights,
                            flees: v.flees,
                            star: v.star.then(|| "wood".to_string()),
                            clear_ms: v.clear_ms,
                        })
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    b.max_distance
                        .cmp(&a.max_distance)
                        .then(a.achieved_at.cmp(&b.achieved_at))
                        .then(a.player_id.cmp(&b.player_id))
                });
                Ok(rows
                    .iter()
                    .position(|r| r.player_id == player_id)
                    .map(|i| (rows[i].clone(), i as i64 + 1)))
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
    /// The classes of a player's heroes by slot (GR-7). Empty string for a slot
    /// whose class was never recorded.
    pub async fn get_hero_classes(&self, player_id: Uuid) -> Result<Vec<String>, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT slot, class_key FROM heroes WHERE player_id = $1 ORDER BY slot",
                )
                .bind(player_id)
                .fetch_all(pool)
                .await?;
                let mut out = Vec::new();
                for r in &rows {
                    let slot: i16 = r.get("slot");
                    let key: String = r.get("class_key");
                    let idx = slot.max(0) as usize;
                    if out.len() <= idx {
                        out.resize(idx + 1, String::new());
                    }
                    out[idx] = key;
                }
                Ok(out)
            }
            Backend::Mem(m) => {
                let m = m.lock().unwrap();
                let mut rows: Vec<(i16, String)> = m
                    .hero_classes
                    .iter()
                    .filter(|((p, _), _)| *p == player_id)
                    .map(|((_, slot), key)| (*slot, key.clone()))
                    .collect();
                rows.sort_by_key(|(slot, _)| *slot);
                let mut out = Vec::new();
                for (slot, key) in rows {
                    let idx = slot.max(0) as usize;
                    if out.len() <= idx {
                        out.resize(idx + 1, String::new());
                    }
                    out[idx] = key;
                }
                Ok(out)
            }
        }
    }

    /// Record that a hero of `class_key` reached `level`, if it beats the account's
    /// previous best. Monotonic: a shallow dive can never lower a record earned deep.
    /// Returns `true` when this call set a new best.
    pub async fn record_class_best(
        &self,
        player_id: Uuid,
        class_key: &str,
        level: i32,
    ) -> Result<bool, DbError> {
        if level <= 0 || class_key.is_empty() {
            return Ok(false);
        }
        match &self.backend {
            Backend::Pg(pool) => {
                let res = sqlx::query(
                    "INSERT INTO class_bests (player_id, class_key, best_level) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, class_key) DO UPDATE SET best_level = $3
                       WHERE class_bests.best_level < $3",
                )
                .bind(player_id)
                .bind(class_key)
                .bind(level)
                .execute(pool)
                .await?;
                Ok(res.rows_affected() > 0)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let e = m.class_bests.entry((player_id, class_key.to_string())).or_insert(0);
                if *e < level {
                    *e = level;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// The best level ever reached per class, for the roster screen and the unlock
    /// rules: `(class_key, best_level)`, deepest first.
    pub async fn get_class_bests(&self, player_id: Uuid) -> Result<Vec<(String, i32)>, DbError> {
        let mut rows = match &self.backend {
            Backend::Pg(pool) => sqlx::query(
                "SELECT class_key, best_level FROM class_bests WHERE player_id = $1",
            )
            .bind(player_id)
            .fetch_all(pool)
            .await?
            .iter()
            .map(|r| (r.get::<String, _>("class_key"), r.get::<i32, _>("best_level")))
            .collect::<Vec<_>>(),
            Backend::Mem(m) => m
                .lock()
                .unwrap()
                .class_bests
                .iter()
                .filter(|((p, _), _)| *p == player_id)
                .map(|((_, k), v)| (k.clone(), *v))
                .collect(),
        };
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(rows)
    }

    /// Grant unlocks, returning the keys that were actually NEW. Idempotent: a
    /// milestone reported twice grants nothing the second time, which is what lets
    /// the game loop fire it freely without tracking whether it already has.
    pub async fn grant_unlocks(
        &self,
        player_id: Uuid,
        keys: &[String],
    ) -> Result<Vec<String>, DbError> {
        let mut granted = Vec::new();
        for key in keys.iter().filter(|k| !k.is_empty()) {
            let is_new = match &self.backend {
                Backend::Pg(pool) => {
                    sqlx::query(
                        "INSERT INTO unlocks (player_id, unlock_key) VALUES ($1, $2)
                         ON CONFLICT (player_id, unlock_key) DO NOTHING",
                    )
                    .bind(player_id)
                    .bind(key)
                    .execute(pool)
                    .await?
                    .rows_affected()
                        > 0
                }
                Backend::Mem(m) => {
                    m.lock().unwrap().unlocks.insert((player_id, key.clone()))
                }
            };
            if is_new {
                granted.push(key.clone());
            }
        }
        Ok(granted)
    }

    /// Everything an account owns. A player with no rows at all still has the
    /// starting set: the registry's `Start` unlocks are implicit, so an account
    /// created before unlocks existed is not locked out of its own Explorer.
    pub async fn get_unlocks(&self, player_id: Uuid) -> Result<Vec<String>, DbError> {
        let mut rows = match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query("SELECT unlock_key FROM unlocks WHERE player_id = $1")
                    .bind(player_id)
                    .fetch_all(pool)
                    .await?
                    .iter()
                    .map(|r| r.get::<String, _>("unlock_key"))
                    .collect::<Vec<_>>()
            }
            Backend::Mem(m) => m
                .lock()
                .unwrap()
                .unlocks
                .iter()
                .filter(|(p, _)| *p == player_id)
                .map(|(_, k)| k.clone())
                .collect(),
        };
        for k in meld_proto::unlocks::starting_unlocks() {
            if !rows.iter().any(|r| r == k) {
                rows.push(k.to_string());
            }
        }
        rows.sort();
        Ok(rows)
    }

    /// Record a hero slot's class (GR-7). Upsert, so the party a player takes on a
    /// dive is what their roster becomes.
    pub async fn set_hero_class(
        &self,
        player_id: Uuid,
        slot: i16,
        class_key: &str,
    ) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "INSERT INTO heroes (player_id, slot, name, class_key) VALUES ($1, $2, '', $3)
                     ON CONFLICT (player_id, slot) DO UPDATE SET class_key = $3",
                )
                .bind(player_id)
                .bind(slot)
                .bind(class_key)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                m.lock()
                    .unwrap()
                    .hero_classes
                    .insert((player_id, slot), class_key.to_string());
            }
        }
        Ok(())
    }

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
                // Seed generated hero names (renameable on the party screen). Seeded
                // off the account so the four are stable and distinct — "Hero 1" read
                // as a form the game forgot to fill in.
                let names = meld_proto::names::roster(&player_id.to_string(), 4);
                sqlx::query(
                    "INSERT INTO heroes (player_id, slot, name)
                     VALUES ($1,0,$2),($1,1,$3),($1,2,$4),($1,3,$5)",
                )
                .bind(player_id)
                .bind(&names[0])
                .bind(&names[1])
                .bind(&names[2])
                .bind(&names[3])
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
                        family: String::new(),
                        armor_weight: String::new(),
                        affixes: "[]".into(),
                        unique_key: String::new(),
                        set_key: String::new(),
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
                for (slot, name) in
                    meld_proto::names::roster(&player_id.to_string(), 4).into_iter().enumerate()
                {
                    m.heroes.insert((player_id, slot as i16), name);
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
    /// Run one recipe: consume `inputs`, add `output`, and credit `skill_xp` to the
    /// Meld skill the RECIPE names — a potion is Alchemy, metalwork is Forging.
    pub async fn craft(
        &self,
        player_id: Uuid,
        inputs: &[(String, i32)],
        output: (&str, i32),
        skill: &str,
        skill_xp: i64,
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
                    "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1, $3, $2)
                     ON CONFLICT (player_id, skill_kind) DO UPDATE SET xp = meld_skills.xp + $2",
                )
                .bind(player_id)
                .bind(skill_xp)
                .bind(skill)
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
                    .entry((player_id, skill.to_string()))
                    .or_insert(0) += skill_xp;
                Ok(true)
            }
        }
    }

    /// Spend `materials` + `chits` and insert one crafted (insured) gear row (MS-1).
    /// Atomic: a smith who cannot pay keeps their materials and gets nothing.
    /// Returns `false` when the cost cannot be met.
    pub async fn forge_gear(
        &self,
        player_id: Uuid,
        materials: &[(String, i32)],
        chits: i64,
        piece: &LootedGear,
    ) -> Result<bool, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                for (kind, need) in materials {
                    let spent = sqlx::query(
                        "UPDATE vault_items SET quantity = quantity - $3
                         WHERE player_id = $1 AND item_kind = $2 AND quantity >= $3",
                    )
                    .bind(player_id)
                    .bind(kind)
                    .bind(need)
                    .execute(&mut *tx)
                    .await?;
                    if spent.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                if chits > 0 {
                    let paid = sqlx::query(
                        "UPDATE vaults SET chits = chits - $2 WHERE player_id = $1 AND chits >= $2",
                    )
                    .bind(player_id)
                    .bind(chits)
                    .execute(&mut *tx)
                    .await?;
                    if paid.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                sqlx::query("DELETE FROM vault_items WHERE player_id = $1 AND quantity <= 0")
                    .bind(player_id)
                    .execute(&mut *tx)
                    .await?;
                // Crafted gear is INSURED: a smith's work survives a death the way
                // anything else bought with the Vault's own resources does.
                sqlx::query(
                    "INSERT INTO gear (gear_id, owner_player_id, name, slot, class_key, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot, damage_modifiers, family, armor_weight, affixes, unique_key, set_key)
                     VALUES ($1, $2, $3, $4, $5, 'blue', $6, $7, $8, $9, $10, $11, NULL, $12, $13, $14, $15, '', '')",
                )
                .bind(piece.gear_id)
                .bind(player_id)
                .bind(&piece.name)
                .bind(&piece.slot)
                .bind(&piece.class_key)
                .bind(piece.tier)
                .bind(piece.atk_bonus)
                .bind(piece.def_bonus)
                .bind(piece.spd_bonus)
                .bind(piece.base_max_durability)
                .bind(piece.max_durability)
                .bind(&piece.damage_modifiers)
                .bind(&piece.family)
                .bind(&piece.armor_weight)
                .bind(&piece.affixes)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(true)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for (kind, need) in materials {
                    let have = m
                        .vault_items
                        .get(&(player_id, kind.clone()))
                        .copied()
                        .unwrap_or(0);
                    if have < *need {
                        return Ok(false);
                    }
                }
                if chits > 0 && m.chits.get(&player_id).copied().unwrap_or(0) < chits {
                    return Ok(false);
                }
                for (kind, need) in materials {
                    let key = (player_id, kind.clone());
                    if let Some(q) = m.vault_items.get_mut(&key) {
                        *q -= *need;
                        if *q <= 0 {
                            m.vault_items.remove(&key);
                        }
                    }
                }
                if chits > 0 {
                    *m.chits.entry(player_id).or_insert(0) -= chits;
                }
                m.gear.insert(
                    piece.gear_id,
                    MemGear {
                        gear_id: piece.gear_id,
                        owner_player_id: player_id,
                        name: piece.name.clone(),
                        slot: piece.slot.clone(),
                        class_key: piece.class_key.clone(),
                        insurance: "blue".into(),
                        tier: piece.tier,
                        atk_bonus: piece.atk_bonus,
                        def_bonus: piece.def_bonus,
                        spd_bonus: piece.spd_bonus,
                        base_max_durability: piece.base_max_durability,
                        max_durability: piece.max_durability,
                        equipped_hero_slot: None,
                        damage_modifiers: piece.damage_modifiers.clone(),
                        family: piece.family.clone(),
                        armor_weight: piece.armor_weight.clone(),
                        affixes: piece.affixes.clone(),
                        unique_key: String::new(),
                        set_key: String::new(),
                    },
                );
                Ok(true)
            }
        }
    }

    /// Read one owned gear row (for a reroll/repair that has to know what it is
    /// working on). `None` when the caller does not own it.
    pub async fn get_gear_by_id(
        &self,
        player_id: Uuid,
        gear_id: Uuid,
    ) -> Result<Option<GearRow>, DbError> {
        Ok(self
            .get_gear(player_id)
            .await?
            .into_iter()
            .find(|g| g.gear_id == gear_id))
    }

    /// Replace one piece's affixes for `materials` + `chits` (MS-1). Atomic, and a
    /// no-op that reports `false` when the smith cannot pay.
    /// Spend materials + chits on a service that leaves no row behind — MS-1's temporary
    /// **enhance**, whose effect lives in the run rather than the Vault. Atomic and
    /// all-or-nothing, so a smith who cannot pay keeps their stock.
    pub async fn spend_for_service(
        &self,
        player_id: Uuid,
        materials: &[(String, i32)],
        chits: i64,
    ) -> Result<bool, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                for (kind, need) in materials {
                    let spent = sqlx::query(
                        "UPDATE vault_items SET quantity = quantity - $3
                         WHERE player_id = $1 AND item_kind = $2 AND quantity >= $3",
                    )
                    .bind(player_id)
                    .bind(kind)
                    .bind(need)
                    .execute(&mut *tx)
                    .await?;
                    if spent.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                sqlx::query("DELETE FROM vault_items WHERE player_id = $1 AND quantity <= 0")
                    .bind(player_id)
                    .execute(&mut *tx)
                    .await?;
                if chits > 0 {
                    let paid = sqlx::query(
                        "UPDATE vaults SET chits = chits - $2 WHERE player_id = $1 AND chits >= $2",
                    )
                    .bind(player_id)
                    .bind(chits)
                    .execute(&mut *tx)
                    .await?;
                    if paid.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                tx.commit().await?;
                Ok(true)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for (kind, need) in materials {
                    if m.vault_items.get(&(player_id, kind.clone())).copied().unwrap_or(0) < *need {
                        return Ok(false);
                    }
                }
                if chits > 0 && m.chits.get(&player_id).copied().unwrap_or(0) < chits {
                    return Ok(false);
                }
                for (kind, need) in materials {
                    let key = (player_id, kind.clone());
                    if let Some(q) = m.vault_items.get_mut(&key) {
                        *q -= *need;
                        if *q <= 0 {
                            m.vault_items.remove(&key);
                        }
                    }
                }
                if chits > 0 {
                    *m.chits.entry(player_id).or_insert(0) -= chits;
                }
                Ok(true)
            }
        }
    }

    pub async fn reroll_gear_affixes(
        &self,
        player_id: Uuid,
        gear_id: Uuid,
        materials: &[(String, i32)],
        chits: i64,
        affixes_json: &str,
    ) -> Result<bool, DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                for (kind, need) in materials {
                    let spent = sqlx::query(
                        "UPDATE vault_items SET quantity = quantity - $3
                         WHERE player_id = $1 AND item_kind = $2 AND quantity >= $3",
                    )
                    .bind(player_id)
                    .bind(kind)
                    .bind(need)
                    .execute(&mut *tx)
                    .await?;
                    if spent.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                if chits > 0 {
                    let paid = sqlx::query(
                        "UPDATE vaults SET chits = chits - $2 WHERE player_id = $1 AND chits >= $2",
                    )
                    .bind(player_id)
                    .bind(chits)
                    .execute(&mut *tx)
                    .await?;
                    if paid.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(false);
                    }
                }
                let hit = sqlx::query(
                    "UPDATE gear SET affixes = $3 WHERE gear_id = $1 AND owner_player_id = $2",
                )
                .bind(gear_id)
                .bind(player_id)
                .bind(affixes_json)
                .execute(&mut *tx)
                .await?;
                if hit.rows_affected() == 0 {
                    tx.rollback().await?;
                    return Ok(false);
                }
                tx.commit().await?;
                Ok(true)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                if !m
                    .gear
                    .get(&gear_id)
                    .map(|g| g.owner_player_id == player_id)
                    .unwrap_or(false)
                {
                    return Ok(false);
                }
                for (kind, need) in materials {
                    if m.vault_items
                        .get(&(player_id, kind.clone()))
                        .copied()
                        .unwrap_or(0)
                        < *need
                    {
                        return Ok(false);
                    }
                }
                if chits > 0 && m.chits.get(&player_id).copied().unwrap_or(0) < chits {
                    return Ok(false);
                }
                for (kind, need) in materials {
                    let key = (player_id, kind.clone());
                    if let Some(q) = m.vault_items.get_mut(&key) {
                        *q -= *need;
                        if *q <= 0 {
                            m.vault_items.remove(&key);
                        }
                    }
                }
                if chits > 0 {
                    *m.chits.entry(player_id).or_insert(0) -= chits;
                }
                if let Some(g) = m.gear.get_mut(&gear_id) {
                    g.affixes = affixes_json.to_string();
                }
                Ok(true)
            }
        }
    }

    /// Repair up to `points` of a piece's lost max durability for `chits` (MS-1 /
    /// GR-2's repair sink). Never exceeds the piece's original
    /// `base_max_durability`. Returns the points actually restored (0 = nothing to
    /// repair, or the smith could not pay).
    pub async fn repair_gear(
        &self,
        player_id: Uuid,
        gear_id: Uuid,
        points: i32,
        chits_per_point: i64,
    ) -> Result<i32, DbError> {
        let Some(row) = self.get_gear_by_id(player_id, gear_id).await? else {
            return Ok(0);
        };
        let missing = (row.base_max_durability - row.max_durability).max(0);
        let restore = missing.min(points.max(0));
        if restore == 0 {
            return Ok(0);
        }
        let cost = chits_per_point * restore as i64;
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                if cost > 0 {
                    let paid = sqlx::query(
                        "UPDATE vaults SET chits = chits - $2 WHERE player_id = $1 AND chits >= $2",
                    )
                    .bind(player_id)
                    .bind(cost)
                    .execute(&mut *tx)
                    .await?;
                    if paid.rows_affected() == 0 {
                        tx.rollback().await?;
                        return Ok(0);
                    }
                }
                sqlx::query(
                    "UPDATE gear SET max_durability = LEAST(max_durability + $3, base_max_durability)
                     WHERE gear_id = $1 AND owner_player_id = $2",
                )
                .bind(gear_id)
                .bind(player_id)
                .bind(restore)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(restore)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                if cost > 0 && m.chits.get(&player_id).copied().unwrap_or(0) < cost {
                    return Ok(0);
                }
                if cost > 0 {
                    *m.chits.entry(player_id).or_insert(0) -= cost;
                }
                if let Some(g) = m.gear.get_mut(&gear_id) {
                    g.max_durability = (g.max_durability + restore).min(g.base_max_durability);
                }
                Ok(restore)
            }
        }
    }

    /// Buy `qty` of `item_kind` from a town vendor for `unit_price` chits each
    /// (EC-2). Atomic: the chits leave the Vault and the goods arrive in the same
    /// transaction, so a failed purchase can never bill a player for nothing.
    /// Returns `false` when they cannot afford it.
    pub async fn buy_from_vendor(
        &self,
        player_id: Uuid,
        item_kind: &str,
        qty: i32,
        unit_price: i64,
    ) -> Result<bool, DbError> {
        if qty <= 0 || unit_price < 0 {
            return Ok(false);
        }
        let cost = unit_price * qty as i64;
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let paid = sqlx::query(
                    "UPDATE vaults SET chits = chits - $2 WHERE player_id = $1 AND chits >= $2",
                )
                .bind(player_id)
                .bind(cost)
                .execute(&mut *tx)
                .await?;
                if paid.rows_affected() == 0 {
                    tx.rollback().await?;
                    return Ok(false);
                }
                sqlx::query(
                    "INSERT INTO vault_items (player_id, item_kind, quantity) VALUES ($1, $2, $3)
                     ON CONFLICT (player_id, item_kind)
                     DO UPDATE SET quantity = vault_items.quantity + $3",
                )
                .bind(player_id)
                .bind(item_kind)
                .bind(qty)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(true)
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let chits = m.chits.entry(player_id).or_insert(0);
                if *chits < cost {
                    return Ok(false);
                }
                *chits -= cost;
                *m.vault_items
                    .entry((player_id, item_kind.to_string()))
                    .or_insert(0) += qty;
                Ok(true)
            }
        }
    }

    /// Sell `qty` of a material out of the Vault for `unit_price` chits each and
    /// credit `skill_xp` to `skill` (MS-1's Broker: the floor price under every
    /// material, and Mercantile's first XP source). Atomic and all-or-nothing — a
    /// seller who is short keeps their stack and earns nothing. Returns the chits
    /// paid, or `None` when the stack cannot cover the sale.
    pub async fn sell_to_vendor(
        &self,
        player_id: Uuid,
        item_kind: &str,
        qty: i32,
        unit_price: i64,
        skill: &str,
        skill_xp: i64,
    ) -> Result<Option<i64>, DbError> {
        if qty <= 0 || unit_price <= 0 {
            return Ok(None);
        }
        let paid = unit_price * qty as i64;
        match &self.backend {
            Backend::Pg(pool) => {
                let mut tx = pool.begin().await?;
                let sold = sqlx::query(
                    "UPDATE vault_items SET quantity = quantity - $3
                     WHERE player_id = $1 AND item_kind = $2 AND quantity >= $3",
                )
                .bind(player_id)
                .bind(item_kind)
                .bind(qty)
                .execute(&mut *tx)
                .await?;
                if sold.rows_affected() == 0 {
                    tx.rollback().await?;
                    return Ok(None);
                }
                sqlx::query("DELETE FROM vault_items WHERE player_id = $1 AND quantity <= 0")
                    .bind(player_id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "INSERT INTO vaults (player_id, chits) VALUES ($1, $2)
                     ON CONFLICT (player_id) DO UPDATE SET chits = vaults.chits + $2",
                )
                .bind(player_id)
                .bind(paid)
                .execute(&mut *tx)
                .await?;
                if skill_xp > 0 {
                    sqlx::query(
                        "INSERT INTO meld_skills (player_id, skill_kind, xp) VALUES ($1, $3, $2)
                         ON CONFLICT (player_id, skill_kind) DO UPDATE SET xp = meld_skills.xp + $2",
                    )
                    .bind(player_id)
                    .bind(skill_xp)
                    .bind(skill)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(Some(paid))
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                let key = (player_id, item_kind.to_string());
                if m.vault_items.get(&key).copied().unwrap_or(0) < qty {
                    return Ok(None);
                }
                let stack = m.vault_items.get_mut(&key).unwrap();
                *stack -= qty;
                if *stack <= 0 {
                    m.vault_items.remove(&key);
                }
                *m.chits.entry(player_id).or_insert(0) += paid;
                if skill_xp > 0 {
                    *m.skills.entry((player_id, skill.to_string())).or_insert(0) += skill_xp;
                }
                Ok(Some(paid))
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
                    "SELECT gear_id, name, slot, class_key, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot, damage_modifiers, family, armor_weight, affixes, unique_key, set_key
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
        // GR-7: a two-handed class has no off-hand, so don't hand it a buckler it
        // would only have to take off to hold its own staff.
        let classes = self.get_hero_classes(player_id).await?;
        let has_off_hand = |slot: i32| -> bool {
            classes
                .get(slot.max(0) as usize)
                .and_then(|k| meld_proto::equipment::class_from_key(k))
                .map(meld_proto::equipment::has_off_hand)
                .unwrap_or(true)
        };
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
                if category == "off_hand" && !has_off_hand(slot) {
                    continue;
                }
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
                            family: String::new(),
                            armor_weight: String::new(),
                            affixes: "[]".into(),
                            unique_key: String::new(),
                            set_key: String::new(),
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
                    insert_gear_row(&mut tx, player_id, g).await?;
                }
                tx.commit().await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for g in gear {
                    // ON CONFLICT (gear_id) DO NOTHING.
                    m.gear.entry(g.gear_id).or_insert_with(|| mem_gear_row(player_id, g));
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
        // GR-5: a hero only benefits from gear its class may actually wear — the
        // authoritative check, since a hero's class is chosen per dive and gear is
        // equipped to a *slot*. A signature piece names its class (`class_key`);
        // ordinary pieces are judged by family / armor weight. An item carrying
        // none of the three descriptors is unrestricted.
        let wearable = |slot: usize,
                        class_key: &str,
                        item_slot: &str,
                        family: &str,
                        weight: &str|
         -> bool {
            let Some(hero_key) = hero_classes.get(slot) else {
                return class_key.is_empty();
            };
            if !class_key.is_empty() && class_key != hero_key.as_str() {
                return false;
            }
            let Some(class) = meld_proto::equipment::class_from_key(hero_key) else {
                return true;
            };
            meld_proto::equipment::check_equip(
                class,
                class_key,
                item_slot,
                meld_proto::equipment::ItemFamily::from_wire(family),
                meld_proto::equipment::ArmorWeight::from_wire(weight),
            ) == meld_proto::equipment::Legality::Ok
        };
        match &self.backend {
            Backend::Pg(pool) => {
                let rows = sqlx::query(
                    "SELECT equipped_hero_slot, atk_bonus, def_bonus, spd_bonus, class_key, max_durability, damage_modifiers, slot, family, armor_weight, affixes, unique_key, set_key FROM gear
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
                    if wearable(
                        slot as usize,
                        &class_key,
                        &row.get::<String, _>("slot"),
                        &row.get::<String, _>("family"),
                        &row.get::<String, _>("armor_weight"),
                    ) {
                        let hero_class = hero_classes.get(slot as usize).cloned();
                        if let Some(b) = bonuses.get_mut(slot as usize) {
                            b.atk += row.get::<i32, _>("atk_bonus");
                            b.def += row.get::<i32, _>("def_bonus");
                            b.spd += row.get::<i32, _>("spd_bonus");
                            append_modifier_entries(
                                &mut b.modifiers,
                                &row.get::<String, _>("damage_modifiers"),
                            );
                            apply_affixes(
                                b,
                                &row.get::<String, _>("affixes"),
                                hero_class.as_deref(),
                            );
                            apply_chase_tiers(
                                b,
                                &row.get::<String, _>("unique_key"),
                                &row.get::<String, _>("set_key"),
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
                        if wearable(slot as usize, &g.class_key, &g.slot, &g.family, &g.armor_weight) {
                            let hero_class = hero_classes.get(slot as usize).cloned();
                            if let Some(b) = bonuses.get_mut(slot as usize) {
                                apply_affixes(b, &g.affixes, hero_class.as_deref());
                                apply_chase_tiers(b, &g.unique_key, &g.set_key);
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
    /// Set one item's insurance tier directly. Test-only: the drop roll is what picks
    /// a tier in the real game, and there is no player-facing way to change one.
    #[cfg(test)]
    pub async fn force_insurance(&self, gear_id: Uuid, tier: &str) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query("UPDATE gear SET insurance = $2 WHERE gear_id = $1")
                    .bind(gear_id)
                    .bind(tier)
                    .execute(pool)
                    .await?;
            }
            Backend::Mem(m) => {
                if let Some(g) = m.lock().unwrap().gear.get_mut(&gear_id) {
                    g.insurance = tier.to_string();
                }
            }
        }
        Ok(())
    }

    pub async fn burn_ephemeral_gear(&self, player_id: Uuid) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query("DELETE FROM gear WHERE owner_player_id = $1 AND insurance = 'red'")
                    .bind(player_id)
                    .execute(pool)
                    .await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                m.gear.retain(|_, g| !(g.owner_player_id == player_id && g.insurance == "red"));
            }
        }
        Ok(())
    }

    /// Destroy the STANDARD gear a player had equipped when a run ended in a wipe.
    ///
    /// The only thing that ever takes standard gear, which is the trade against
    /// insured: normal kit is untouched right up until one bad night takes all of it,
    /// insured kit can never be taken but is never quite whole. Equipped only — what
    /// they left at home was never at risk.
    pub async fn destroy_equipped_standard_gear(&self, player_id: Uuid) -> Result<(), DbError> {
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "DELETE FROM gear
                     WHERE owner_player_id = $1 AND insurance = 'standard'
                       AND equipped_hero_slot IS NOT NULL",
                )
                .bind(player_id)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                m.gear.retain(|_, g| {
                    !(g.owner_player_id == player_id
                        && g.insurance == "standard"
                        && g.equipped_hero_slot.is_some())
                });
            }
        }
        Ok(())
    }

    /// Apply the death durability sink to equipped blue-chest gear:
    /// `max_durability ← floor(max_durability × 0.9)` (CANON.md D6).
    pub async fn apply_death_durability(
        &self,
        player_id: Uuid,
        rate: f64,
    ) -> Result<(), DbError> {
        let keep = (1.0 - rate).clamp(0.0, 1.0);
        match &self.backend {
            Backend::Pg(pool) => {
                sqlx::query(
                    "UPDATE gear SET max_durability = FLOOR(max_durability * $2)
                     WHERE owner_player_id = $1 AND insurance = 'blue' AND equipped_hero_slot IS NOT NULL",
                )
                .bind(player_id)
                .bind(keep)
                .execute(pool)
                .await?;
            }
            Backend::Mem(m) => {
                let mut m = m.lock().unwrap();
                for g in m.gear.values_mut() {
                    if g.owner_player_id == player_id && g.insurance == "blue" && g.equipped_hero_slot.is_some() {
                        g.max_durability = (g.max_durability as f64 * keep).floor() as i32;
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
    /// The equip-time legality verdict for putting `gear` on `hero_slot` (GR-5 +
    /// GR-7). `None` when the hero's class was never recorded — then the equip is
    /// allowed and derivation stays the backstop, so a player is never locked out
    /// of their own Vault by missing data.
    fn equip_legality(
        hero_class: Option<&str>,
        item_slot: &str,
        item_class_key: &str,
        family: &str,
        armor_weight: &str,
    ) -> Option<meld_proto::equipment::Legality> {
        use meld_proto::equipment as eq;
        let class = eq::class_from_key(hero_class.unwrap_or(""))?;
        Some(eq::check_equip(
            class,
            item_class_key,
            item_slot,
            eq::ItemFamily::from_wire(family),
            eq::ArmorWeight::from_wire(armor_weight),
        ))
    }

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
                    "SELECT slot, max_durability, equipped_hero_slot, class_key, family, armor_weight FROM gear
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
                let item_class_key: String = row.get("class_key");
                let family: String = row.get("family");
                let armor_weight: String = row.get("armor_weight");

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
                // GR-5 legality, now checkable because the hero's class is persisted
                // (GR-7). Derivation still refuses to pay out illegal gear; this is
                // what turns a silent no-benefit equip into an answer.
                let hero_class: Option<String> = sqlx::query_scalar(
                    "SELECT class_key FROM heroes WHERE player_id = $1 AND slot = $2",
                )
                .bind(player_id)
                .bind(hero_slot as i16)
                .fetch_optional(&mut *tx)
                .await?
                .filter(|k: &String| !k.is_empty());
                if let Some(verdict) = Self::equip_legality(
                    hero_class.as_deref(),
                    &slot,
                    &item_class_key,
                    &family,
                    &armor_weight,
                ) {
                    if verdict != meld_proto::equipment::Legality::Ok {
                        tx.rollback().await?;
                        return Ok(EquipResult::ClassLocked(verdict));
                    }
                }
                // Both hands or neither: a 2H weapon cannot share the hero with an
                // off-hand item, in either order of equipping.
                let two_handed = meld_proto::equipment::ItemFamily::from_wire(&family)
                    .map(|f| f.reserves_off_hand())
                    .unwrap_or(false);
                if two_handed || slot == "off_hand" {
                    let blocking: i64 = if two_handed {
                        sqlx::query_scalar(
                            "SELECT COUNT(*) FROM gear
                             WHERE owner_player_id = $1 AND equipped_hero_slot = $2
                               AND slot = 'off_hand' AND gear_id <> $3",
                        )
                        .bind(player_id)
                        .bind(hero_slot)
                        .bind(gear_id)
                        .fetch_one(&mut *tx)
                        .await?
                    } else {
                        sqlx::query_scalar(
                            "SELECT COUNT(*) FROM gear
                             WHERE owner_player_id = $1 AND equipped_hero_slot = $2
                               AND slot = 'main_hand' AND family IN ('spear','staff','globe')
                               AND gear_id <> $3",
                        )
                        .bind(player_id)
                        .bind(hero_slot)
                        .bind(gear_id)
                        .fetch_one(&mut *tx)
                        .await?
                    };
                    if blocking > 0 {
                        tx.rollback().await?;
                        return Ok(EquipResult::TwoHandedConflict);
                    }
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
                    // A FULL slot is the normal case — every hero starts dressed — so
                    // refusing here made the equip picker a dead end: every press came back
                    // 409, and the player was never shown the reason, so it read as a dead
                    // button. Putting a sword on means taking the old one off; that is not a
                    // decision worth interrupting for. A multi-capacity category still
                    // refuses, because with two accessory slots full the player is choosing
                    // WHICH one comes off, and we must not choose for them.
                    if category_capacity(&slot) > 1 {
                        tx.rollback().await?;
                        return Ok(EquipResult::SlotOccupied);
                    }
                    sqlx::query(
                        "UPDATE gear SET equipped_hero_slot = NULL
                         WHERE owner_player_id = $1 AND slot = $2 AND equipped_hero_slot = $3
                           AND gear_id <> $4",
                    )
                    .bind(player_id)
                    .bind(&slot)
                    .bind(hero_slot)
                    .bind(gear_id)
                    .execute(&mut *tx)
                    .await?;
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
                let Some((slot, max_durability, already, item_class_key, family, armor_weight)) = m
                    .gear
                    .get(&gear_id)
                    .filter(|g| g.owner_player_id == player_id)
                    .map(|g| {
                        (
                            g.slot.clone(),
                            g.max_durability,
                            g.equipped_hero_slot,
                            g.class_key.clone(),
                            g.family.clone(),
                            g.armor_weight.clone(),
                        )
                    })
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
                let hero_class = m
                    .hero_classes
                    .get(&(player_id, hero_slot as i16))
                    .filter(|k| !k.is_empty())
                    .cloned();
                if let Some(verdict) = Self::equip_legality(
                    hero_class.as_deref(),
                    &slot,
                    &item_class_key,
                    &family,
                    &armor_weight,
                ) {
                    if verdict != meld_proto::equipment::Legality::Ok {
                        return Ok(EquipResult::ClassLocked(verdict));
                    }
                }
                let two_handed = meld_proto::equipment::ItemFamily::from_wire(&family)
                    .map(|f| f.reserves_off_hand())
                    .unwrap_or(false);
                let blocked = m.gear.values().any(|g| {
                    g.owner_player_id == player_id
                        && g.equipped_hero_slot == Some(hero_slot)
                        && g.gear_id != gear_id
                        && if two_handed {
                            g.slot == "off_hand"
                        } else {
                            slot == "off_hand"
                                && g.slot == "main_hand"
                                && meld_proto::equipment::ItemFamily::from_wire(&g.family)
                                    .map(|f| f.reserves_off_hand())
                                    .unwrap_or(false)
                        }
                });
                if blocked {
                    return Ok(EquipResult::TwoHandedConflict);
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
                    if category_capacity(&slot) > 1 {
                        return Ok(EquipResult::SlotOccupied);
                    }
                    // Displace the occupant — see the Pg arm for why a full slot swaps.
                    let displaced: Vec<Uuid> = m
                        .gear
                        .values()
                        .filter(|g| {
                            g.owner_player_id == player_id
                                && g.slot == slot
                                && g.equipped_hero_slot == Some(hero_slot)
                                && g.gear_id != gear_id
                        })
                        .map(|g| g.gear_id)
                        .collect();
                    for id in displaced {
                        if let Some(g) = m.gear.get_mut(&id) {
                            g.equipped_hero_slot = None;
                        }
                    }
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
    /// The hero's class may not wear this item (GR-5) → 409 conflict. Carries the
    /// rule that failed so the UI can say *why*, not just "cannot equip".
    ClassLocked(meld_proto::equipment::Legality),
    /// A two-handed weapon needs both hands: either this 2H weapon and a filled
    /// off-hand, or an off-hand item while a 2H weapon is held → 409 conflict.
    TwoHandedConflict,
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
    /// AD-1 ward affixes: what the hero *starts each battle* holding.
    pub barrier: i32,
    pub regen: i32,
    /// Evasion percentage points.
    pub evasion: i32,
    /// AD-1 keyword affixes (class-mechanic twists), already filtered to this
    /// hero's class: banked Adrenaline at battle start, extra Focus slots.
    pub adrenaline: i32,
    pub focus_slots: i32,
    /// AD-3 brand: the element this hero's attacks deal. The first branded weapon
    /// wins — two brands would mean an attack with two types, which the engine's
    /// one-type-per-effect model has no answer for.
    pub brand: Option<String>,
    /// AD-1 unique drawbacks, already summed: what this loadout *costs*.
    pub penalty_atk: i32,
    pub penalty_def: i32,
    pub penalty_spd: i32,
    pub penalty_max_hp: i32,
    /// AD-1 set pieces worn by this hero: (set key, count). Battle assembly turns
    /// the completed ones into a PARTY-wide bonus.
    pub set_pieces: Vec<(String, usize)>,
    /// Synergy affixes that have not been resolved yet: (ally class key, atk, def).
    /// Battle assembly knows the party composition, so it decides which of these
    /// actually pay out — a drop that asks for an ally is a *party* build decision.
    pub synergies: Vec<(String, i32, i32)>,
    /// Raw per-item elemental entries (DamageType wire key → multiplier) from
    /// every equipped piece — folded (`1 + Σ(mᵢ−1)`) and clamped to 0.0–2.0 at
    /// battle assembly (spec §5 stat aggregation).
    pub modifiers: Vec<(String, f64)>,
}

/// Fold one item's AD-1 affixes into a hero's running bonus.
///
/// Stat and ward affixes apply immediately; a **keyword** affix only counts for
/// the class whose mechanic it twists; a **synergy** affix is deferred to battle
/// assembly, which is the only place that knows the party composition.
fn apply_affixes(b: &mut GearBonus, raw: &str, hero_class: Option<&str>) {
    let class = hero_class.and_then(meld_proto::equipment::class_from_key);
    for a in meld_proto::affixes::from_json(raw) {
        if let Some(c) = class {
            if !a.applies_to(c) {
                continue;
            }
        } else if a.def().and_then(|d| d.only_class).is_some() {
            continue;
        }
        let m = a.magnitude;
        match a.key.as_str() {
            "atk" => b.atk += m,
            "def" => b.def += m,
            "spd" => b.spd += m,
            "barrier" => b.barrier += m,
            "regen" => b.regen += m,
            "evasion" => b.evasion += m,
            "adrenaline_primed" => b.adrenaline += m,
            "focus_slot" => b.focus_slots += m,
            "brand" if b.brand.is_none() => b.brand = a.element.clone(),
            "resist" => {
                if let Some(el) = &a.element {
                    // A resist affix reads as a percentage; the modifier plumbing
                    // wants a multiplier (25% resisted → 0.75).
                    let mult = 1.0 - (m.clamp(0, 100) as f64 / 100.0);
                    b.modifiers.push((el.clone(), mult));
                }
            }
            "ally_atk" | "ally_def" => {
                if let Some(ally) = &a.ally_class {
                    let (atk, def) = if a.key == "ally_atk" { (m, 0) } else { (0, m) };
                    b.synergies.push((ally.clone(), atk, def));
                }
            }
            _ => {}
        }
    }
}

/// Fold one item's AD-1 chase-tier facts into a hero's running bonus: a unique's
/// **drawback** (its affixes already came through `apply_affixes`, since they are
/// stored on the row like any other) and its set membership.
///
/// Set *counting* happens per hero here; the payout is party-wide and therefore
/// belongs to battle assembly, which is the only place that sees the whole party.
fn apply_chase_tiers(b: &mut GearBonus, unique_key: &str, set_key: &str) {
    use meld_proto::uniques::{self as uq, Drawback};
    if let Some(u) = uq::unique(unique_key) {
        match u.drawback {
            Drawback::Atk(n) => b.penalty_atk += n,
            Drawback::Def(n) => b.penalty_def += n,
            Drawback::Spd(n) => b.penalty_spd += n,
            Drawback::MaxHp(n) => b.penalty_max_hp += n,
        }
    }
    if !set_key.is_empty() && uq::set(set_key).is_some() {
        match b.set_pieces.iter_mut().find(|(k, _)| k == set_key) {
            Some((_, count)) => *count += 1,
            None => b.set_pieces.push((set_key.to_string(), 1)),
        }
    }
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
    /// GR-5 weapon family wire word; empty = unrestricted.
    pub family: String,
    /// GR-5 armor weight wire word; empty = unrestricted.
    pub armor_weight: String,
    /// AD-1 affixes as JSON (`[]` for none).
    pub affixes: String,
    /// AD-1 unique key; empty for ordinary loot.
    /// Which tier this dropped as — it decides how the item is lost, not just how it
    /// is coloured.
    pub insurance: meld_proto::Insurance,
    pub unique_key: String,
    /// AD-1 set key; empty when not part of a set.
    pub set_key: String,
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
    /// GR-5 weapon family wire word (`sword`, `staff`, …); empty = unrestricted.
    pub family: String,
    /// GR-5 armor weight wire word (`heavy`, `robe`, …); empty = unrestricted.
    pub armor_weight: String,
    /// AD-1 affixes as stored JSON.
    pub affixes: String,
    /// AD-1 unique key; empty for ordinary loot.
    pub unique_key: String,
    /// AD-1 set key; empty when not part of a set.
    pub set_key: String,
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
        family: row.get("family"),
        armor_weight: row.get("armor_weight"),
        affixes: row.get("affixes"),
        unique_key: row.get("unique_key"),
        set_key: row.get("set_key"),
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
    /// heroes.class_key, keyed by (player_id, slot); absent = not yet chosen.
    hero_classes: HashMap<(Uuid, i16), String>,
    /// class_bests.best_level, keyed by (player_id, class_key).
    class_bests: HashMap<(Uuid, String), i32>,
    /// unlocks: the (player, unlock_key) pairs an account owns.
    unlocks: HashSet<(Uuid, String)>,
    /// vanguard (max_distance, achieved_at), keyed by (season, player_id).
    vanguard: HashMap<(i32, Uuid), MemVanguard>,
    /// hunts (progress, claimed), keyed by (player_id, hunt_key).
    hunts: HashMap<(Uuid, String), (i32, bool)>,
    /// bounties, keyed by bounty_id.
    bounties: HashMap<Uuid, MemBounty>,
    /// party_loadouts.classes, keyed by (player_id, name).
    loadouts: HashMap<(Uuid, String), Loadout>,
}

struct MemBounty {
    bounty_id: Uuid,
    player_id: Uuid,
    spec: String,
    state: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
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
    family: String,
    armor_weight: String,
    affixes: String,
    unique_key: String,
    set_key: String,
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
            family: self.family.clone(),
            armor_weight: self.armor_weight.clone(),
            affixes: self.affixes.clone(),
            unique_key: self.unique_key.clone(),
            set_key: self.set_key.clone(),
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
        assert!(db.craft(p, &[("iron".into(), 4)], ("blade", 1), "forging", 5).await.unwrap());
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
        assert!(!db.craft(p, &[("wood".into(), 99)], ("plank", 1), "forging", 5).await.unwrap());
        let (_, items) = db.get_vault(p).await.unwrap();
        assert_eq!(items, vec![("blade".to_string(), 1), ("wood".to_string(), 2)]);
    }

    #[tokio::test]
    async fn selling_a_stack_to_the_broker_pays_chits_and_credits_mercantile() {
        async fn skill_xp(db: &Db, p: Uuid, want: &str) -> i64 {
            db.get_skills(p)
                .await
                .unwrap()
                .into_iter()
                .find(|(k, _)| k == want)
                .map(|(_, xp)| xp)
                .unwrap_or(0)
        }
        let db = mem().await;
        let p = db.register("seller", "pw").await.unwrap().player_id;
        db.bank_extraction(p, &[("bog_ichor".into(), 5)], 0).await.unwrap();

        let paid = db
            .sell_to_vendor(p, "bog_ichor", 3, 20, "mercantile", 8)
            .await
            .unwrap()
            .expect("the sale went through");
        assert_eq!(paid, 60);
        let (chits, items) = db.get_vault(p).await.unwrap();
        assert_eq!(chits, 60);
        assert_eq!(items, vec![("bog_ichor".to_string(), 2)]);
        assert_eq!(skill_xp(&db, p, "mercantile").await, 8);

        // Selling more than you hold is refused whole — no partial sale, no chits.
        assert!(db.sell_to_vendor(p, "bog_ichor", 9, 20, "mercantile", 8).await.unwrap().is_none());
        assert_eq!(db.get_vault(p).await.unwrap(), (60, vec![("bog_ichor".to_string(), 2)]));
        assert_eq!(skill_xp(&db, p, "mercantile").await, 8, "a failed sale paid XP");

        // Selling out empties the stack rather than leaving a zero row behind.
        assert_eq!(
            db.sell_to_vendor(p, "bog_ichor", 2, 20, "mercantile", 8).await.unwrap(),
            Some(40)
        );
        assert_eq!(db.get_vault(p).await.unwrap(), (100, vec![]));
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
            insurance: meld_proto::Insurance::Ephemeral,
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
                family: String::new(),
                armor_weight: String::new(),
                affixes: "[]".into(),
            unique_key: String::new(),
            set_key: String::new(),
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

        // A hero's hand is ALREADY full — the starter kit dresses every one of them — and
        // putting a sword on means taking the old one off. Refusing here made the equip
        // picker a dead end: every press was a 409 on a slot that is always occupied.
        assert_eq!(db.set_equipped(p, looted, Some(0)).await.unwrap(), EquipResult::Ok);
        let after = db.get_gear(p).await.unwrap();
        assert_eq!(
            after.iter().filter(|g| g.slot == "main_hand" && g.equipped_hero_slot == Some(0)).count(),
            1,
            "a swap leaves ONE weapon in the hand, not two"
        );
        assert_eq!(
            after.iter().find(|g| g.gear_id == starter).unwrap().equipped_hero_slot,
            None,
            "the displaced starter goes back to the Vault rather than vanishing"
        );
        // Put it back so the rest of this test reads against the starter kit as before.
        assert_eq!(db.set_equipped(p, starter, Some(0)).await.unwrap(), EquipResult::Ok);
        // Per-character equip: the looted sword goes on hero 1, and hero 0 keeps the
        // starter — two heroes with two different weapons is the point of the feature.
        assert_eq!(db.set_equipped(p, looted, Some(1)).await.unwrap(), EquipResult::Ok);
        let bonuses = db.equipped_gear_bonuses(p, 4, &[]).await.unwrap();
        assert_eq!(bonuses[0].atk, 3);
        assert_eq!(bonuses[1].atk, 7);
        assert_eq!(db.set_equipped(p, Uuid::now_v7(), Some(0)).await.unwrap(), EquipResult::NotFound);

        // Death sink only touches equipped blue-chest gear (looted sword is red).
        db.apply_death_durability(p, 0.1).await.unwrap();
        let starter_row = db.get_gear(p).await.unwrap().into_iter().find(|g| g.gear_id == starter).unwrap();
        assert_eq!(starter_row.max_durability, 90); // floor(100 * 0.9)

        // Spec §5 red-gear canon gap: equipped red gear is DELETED on a run
        // that ends died/abandoned (the looted sword is equipped on hero 1),
        // while blue gear survives (decayed above, never deleted).
        db.burn_ephemeral_gear(p).await.unwrap();
        let after = db.get_gear(p).await.unwrap();
        assert!(after.iter().all(|g| g.name != "Looted Sword"), "equipped red gear burned");
        assert!(after.iter().any(|g| g.gear_id == starter), "blue starter survives");
    }

    #[tokio::test]
    async fn the_three_gear_tiers_are_lost_in_three_different_ways() {
        let db = mem().await;
        let p = db.register("tiers", "correct-horse-battery").await.unwrap().player_id;
        let piece = |name: &str| LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::now_v7(),
            name: name.into(),
            slot: "accessory".into(),
            class_key: String::new(),
            tier: 1,
            atk_bonus: 1,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 100,
            max_durability: 100,
            damage_modifiers: "{}".into(),
            family: String::new(),
            armor_weight: String::new(),
            affixes: "[]".into(),
            unique_key: String::new(),
            set_key: String::new(),
        };
        db.insert_looted_gear(
            p,
            &[piece("Insured Ring"), piece("Burning Ring"), piece("Plain Ring")],
        )
        .await
        .unwrap();
        let ids: Vec<(String, Uuid)> = db
            .get_gear(p)
            .await
            .unwrap()
            .into_iter()
            .map(|g| (g.name, g.gear_id))
            .collect();
        let id_of = |want: &str| ids.iter().find(|(n, _)| n == want).expect("ring exists").1;
        db.force_insurance(id_of("Insured Ring"), "blue").await.unwrap();
        db.force_insurance(id_of("Burning Ring"), "red").await.unwrap();
        db.force_insurance(id_of("Plain Ring"), "standard").await.unwrap();
        // One ring per hero: an accessory slot holds one item, so stacking all three on
        // hero 0 would leave two unequipped and out of the sinks' reach.
        for (slot, want) in ["Insured Ring", "Burning Ring", "Plain Ring"].iter().enumerate() {
            assert_eq!(
                db.set_equipped(p, id_of(want), Some(slot as i32)).await.unwrap(),
                EquipResult::Ok,
                "{want} should equip on hero {slot}"
            );
        }
        let insured = id_of("Insured Ring");
        let has = |gs: &[GearRow], n: &str| gs.iter().any(|g| g.name == n);

        // REACHING THE CITY: only ephemeral burns, and it burns however you got home.
        db.burn_ephemeral_gear(p).await.unwrap();
        let after = db.get_gear(p).await.unwrap();
        assert!(!has(&after, "Burning Ring"), "ephemeral must not survive the trip home");
        assert!(has(&after, "Plain Ring"), "standard gear is yours to keep");
        assert!(has(&after, "Insured Ring"), "insured gear comes home");
        let dur = after.iter().find(|g| g.gear_id == insured).unwrap().max_durability;
        assert_eq!(dur, 100, "surviving a run costs insured gear nothing");

        // A WIPE: standard is destroyed outright, insured only wears down.
        db.destroy_equipped_standard_gear(p).await.unwrap();
        db.apply_death_durability(p, 0.08).await.unwrap();
        let after = db.get_gear(p).await.unwrap();
        assert!(!has(&after, "Plain Ring"), "a wipe takes standard gear entirely");
        assert!(has(&after, "Insured Ring"), "insured gear cannot be taken");
        let dur = after.iter().find(|g| g.gear_id == insured).unwrap().max_durability;
        assert_eq!(dur, 92, "insured gear pays in durability instead: floor(100 * 0.92)");

        // Enough wipes and it finally breaks — the price of never losing it.
        for _ in 0..80 {
            db.apply_death_durability(p, 0.08).await.unwrap();
        }
        let dur = db
            .get_gear(p)
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.gear_id == insured)
            .unwrap()
            .max_durability;
        assert_eq!(dur, 0, "insured gear eventually wears out completely");
    }

    #[tokio::test]
    async fn two_accessories_but_only_one_of_everything_else() {
        let db = mem().await;
        let p = db.register("erin", "pw").await.unwrap().player_id;
        let ring = |name: &str| LootedGear {
            insurance: meld_proto::Insurance::Standard,
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
            family: String::new(),
            armor_weight: String::new(),
            affixes: "[]".into(),
        unique_key: String::new(),
        set_key: String::new(),
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
        assert!(!db.get_hero_rows(p).await.unwrap()[2]);
    }

    #[tokio::test]
    async fn a_hunt_completes_once_and_pays_once() {
        let db = mem().await;
        let p = db
            .register("hunter", &Uuid::new_v4().to_string())
            .await
            .unwrap()
            .player_id;

        assert!(db.get_hunts(p).await.unwrap().is_empty());
        // Progress accumulates and is capped at the target: an overshoot on the last
        // kill cannot bank credit toward a hunt nobody has posted yet.
        assert_eq!(
            db.credit_hunt(p, "cull_the_bloom", 2, 3).await.unwrap(),
            HuntCredit { progress: 2, completed: false }
        );
        assert_eq!(
            db.credit_hunt(p, "cull_the_bloom", 5, 3).await.unwrap(),
            HuntCredit { progress: 3, completed: true }
        );
        // `completed` is the crossing, not the state — a later kill announces nothing.
        assert_eq!(
            db.credit_hunt(p, "cull_the_bloom", 1, 3).await.unwrap(),
            HuntCredit { progress: 3, completed: false }
        );

        assert_eq!(
            db.claim_hunt(p, "cull_the_bloom", 3, 250, Some(("bloom_herb", 2)), None).await.unwrap(),
            HuntClaim::Paid { chits: 250 }
        );
        assert_eq!(
            db.claim_hunt(p, "cull_the_bloom", 3, 250, Some(("bloom_herb", 2)), None).await.unwrap(),
            HuntClaim::AlreadyClaimed
        );
        let (chits, items) = db.get_vault(p).await.unwrap();
        assert_eq!(chits, 250, "the board paid exactly once");
        assert_eq!(items.iter().find(|(k, _)| k == "bloom_herb").map(|(_, q)| *q), Some(2));

        let rows = db.get_hunts(p).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].claimed);
    }

    #[tokio::test]
    async fn a_contract_pays_once_and_the_hunter_rank_only_moves_when_it_does() {
        let db = mem().await;
        let p = db
            .register("den", &Uuid::new_v4().to_string())
            .await
            .unwrap()
            .player_id;
        let id = Uuid::now_v7();
        let now = Utc::now();
        db.insert_bounty(p, id, "{}", now + chrono::Duration::hours(5)).await.unwrap();

        // Standing, so it cannot be claimed and the rank has not moved.
        assert_eq!(
            db.claim_bounty(p, id, 500, None, None, 90).await.unwrap(),
            BountyClaim::NotCompleted
        );
        assert!(db.get_skills(p).await.unwrap().iter().all(|(k, _)| k != "hunting"));

        assert!(db.complete_bounty(p, id).await.unwrap());
        assert!(!db.complete_bounty(p, id).await.unwrap(), "felled twice");
        assert_eq!(
            db.claim_bounty(p, id, 500, Some(("frost_shard", 3)), None, 90).await.unwrap(),
            BountyClaim::Paid { chits: 500 }
        );
        assert_eq!(
            db.claim_bounty(p, id, 500, Some(("frost_shard", 3)), None, 90).await.unwrap(),
            BountyClaim::AlreadyClaimed
        );
        let (chits, items) = db.get_vault(p).await.unwrap();
        assert_eq!(chits, 500, "the Den paid exactly once");
        assert_eq!(items.iter().find(|(k, _)| k == "frost_shard").map(|(_, q)| *q), Some(3));
        // The rank rides the `hunting` skill, banked in the same breath as the payout.
        let xp = db
            .get_skills(p)
            .await
            .unwrap()
            .into_iter()
            .find(|(k, _)| k == "hunting")
            .map(|(_, xp)| xp);
        assert_eq!(xp, Some(90), "the rank moved by more or less than one contract");

        // Another player cannot claim it, and an unknown id is not a payout.
        let q = db.register("poacher", &Uuid::new_v4().to_string()).await.unwrap().player_id;
        assert_eq!(
            db.claim_bounty(q, id, 500, None, None, 90).await.unwrap(),
            BountyClaim::Missing
        );
        assert_eq!(
            db.claim_bounty(p, Uuid::now_v7(), 500, None, None, 90).await.unwrap(),
            BountyClaim::Missing
        );
    }

    #[tokio::test]
    async fn only_a_standing_contract_expires() {
        let db = mem().await;
        let p = db
            .register("window", &Uuid::new_v4().to_string())
            .await
            .unwrap()
            .player_id;
        let now = Utc::now();
        let (stale, fresh, felled) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        db.insert_bounty(p, stale, "{}", now - chrono::Duration::hours(1)).await.unwrap();
        db.insert_bounty(p, fresh, "{}", now + chrono::Duration::hours(1)).await.unwrap();
        db.insert_bounty(p, felled, "{}", now - chrono::Duration::hours(1)).await.unwrap();
        db.complete_bounty(p, felled).await.unwrap();

        assert_eq!(db.expire_bounties(p, now).await.unwrap(), 1);
        let rows = db.list_bounties(p).await.unwrap();
        let state = |id: Uuid| {
            rows.iter().find(|r| r.bounty_id == id).map(|r| r.state.clone()).unwrap()
        };
        assert_eq!(state(stale), "expired");
        assert_eq!(state(fresh), "active", "a live window was withdrawn");
        // A mark already down is owed its reward however long the walk home takes.
        assert_eq!(state(felled), "completed");
    }

    #[tokio::test]
    async fn a_deep_hunt_hands_over_its_piece_exactly_once() {
        let db = mem().await;
        let p = db
            .register("deephunt", &Uuid::new_v4().to_string())
            .await
            .unwrap()
            .player_id;
        let before = db.get_gear(p).await.unwrap().len();
        let piece = LootedGear {
            insurance: meld_proto::enums::Insurance::Insured,
            gear_id: Uuid::now_v7(),
            name: "Keeper's Reward".into(),
            slot: "main_hand".into(),
            class_key: "hunter".into(),
            tier: 3,
            atk_bonus: 12,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 100,
            max_durability: 100,
            damage_modifiers: "{}".into(),
            family: "sword".into(),
            armor_weight: String::new(),
            affixes: "[]".into(),
            unique_key: String::new(),
            set_key: String::new(),
        };

        db.credit_hunt(p, "unseat_the_keeper", 1, 1).await.unwrap();
        assert_eq!(
            db.claim_hunt(p, "unseat_the_keeper", 1, 500, None, Some(&piece)).await.unwrap(),
            HuntClaim::Paid { chits: 500 }
        );
        let after = db.get_gear(p).await.unwrap();
        assert_eq!(after.len(), before + 1, "the piece did not land");
        let awarded = after.iter().find(|g| g.gear_id == piece.gear_id).unwrap();
        assert_eq!(awarded.name, "Keeper's Reward");
        assert!(awarded.equipped_hero_slot.is_none(), "an awarded piece arrives in the Vault");

        // The second press is refused, and it does not mint a second copy.
        assert_eq!(
            db.claim_hunt(p, "unseat_the_keeper", 1, 500, None, Some(&piece)).await.unwrap(),
            HuntClaim::AlreadyClaimed
        );
        assert_eq!(db.get_gear(p).await.unwrap().len(), before + 1);
        assert_eq!(db.get_vault(p).await.unwrap().0, 500);
    }

    #[tokio::test]
    async fn an_unearned_hunt_pays_nothing_and_a_claimed_one_stops_counting() {
        let db = mem().await;
        let p = db
            .register("unearned", &Uuid::new_v4().to_string())
            .await
            .unwrap()
            .player_id;

        db.credit_hunt(p, "unseat_the_keeper", 1, 3).await.unwrap();
        assert_eq!(
            db.claim_hunt(p, "unseat_the_keeper", 3, 500, None, None).await.unwrap(),
            HuntClaim::NotEarned { progress: 1 }
        );
        assert_eq!(db.get_vault(p).await.unwrap().0, 0, "a refusal costs the board nothing");

        // Once claimed, further credit is frozen: re-earning a one-off payout is how a
        // board gets farmed.
        db.credit_hunt(p, "unseat_the_keeper", 2, 3).await.unwrap();
        db.claim_hunt(p, "unseat_the_keeper", 3, 500, None, None).await.unwrap();
        assert_eq!(
            db.credit_hunt(p, "unseat_the_keeper", 3, 3).await.unwrap(),
            HuntCredit { progress: 3, completed: false }
        );
        assert_eq!(db.get_vault(p).await.unwrap().0, 500);
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

        assert!(db.record_vanguard_distance(deep.player_id, season, 400, 0, 0, 0).await.unwrap());
        assert!(db.record_vanguard_distance(shallow.player_id, season, 120, 0, 0, 0).await.unwrap());
        // A shallower run never replaces a deeper record, and reports no new best.
        assert!(!db.record_vanguard_distance(deep.player_id, season, 200, 0, 0, 0).await.unwrap());
        assert!(db.record_vanguard_distance(deep.player_id, season, 900, 0, 0, 0).await.unwrap());
        // Distance 0 (never left the hub) does not put you on the board at all.
        let never = db.register("vg_never", &never_password).await.unwrap();
        assert!(!db.record_vanguard_distance(never.player_id, season, 0, 0, 0, 0).await.unwrap());

        let board = db.vanguard_board(season, 100).await.unwrap();
        assert_eq!(board.len(), 2);
        assert_eq!(board[0].username, "vg_deep");
        assert_eq!(board[0].max_distance, 900);
        assert_eq!(board[1].username, "vg_shallow");
        // Seasons are separate boards: last season's rows never leak into this one.
        assert!(db.vanguard_board(season - 1, 100).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_player_below_the_board_cut_still_has_a_placement() {
        // Searching the LIMITed board for the caller reads as unranked for everyone
        // outside the top page — the one player a placement endpoint exists to serve.
        // Rank is against the whole season.
        let db = mem().await;
        let season = current_season();
        let mut last = Uuid::nil();
        for i in 0..12 {
            let p = db
                .register(&format!("vg_cut{i}"), &Uuid::new_v4().to_string())
                .await
                .unwrap();
            db.record_vanguard_distance(p.player_id, season, 1000 - i * 10, 0, 0, 0)
                .await
                .unwrap();
            last = p.player_id;
        }
        // A three-deep board cannot see the twelfth player at all…
        let page = db.vanguard_board(season, 3).await.unwrap();
        assert_eq!(page.len(), 3);
        assert!(!page.iter().any(|r| r.player_id == last));

        // …but their placement resolves, with their true season-wide rank.
        let (row, rank) = db
            .vanguard_placement(season, last)
            .await
            .unwrap()
            .expect("the deepest-but-last player is still ranked");
        assert_eq!(rank, 12, "rank counts the whole season, not the page");
        assert_eq!(row.max_distance, 1000 - 11 * 10);

        // Someone who never posted has no placement rather than a bogus last place.
        let ghost = db
            .register("vg_ghost", &Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert!(db
            .vanguard_placement(season, ghost.player_id)
            .await
            .unwrap()
            .is_none());
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

    #[tokio::test]
    async fn a_hero_gains_nothing_from_gear_its_class_cannot_wear() {
        let db = mem().await;
        let p = db.register("gr5", "pw").await.unwrap().player_id;
        // Hero 0 is a Resonant (staff only); hero 3 an Explorer (spear is legal).
        let classes: Vec<String> = ["resonant", "phoenix_guard", "psyker", "explorer"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        // The starter kit fills every slot, so free the main hand of both heroes
        // first — otherwise the equip is a no-op and the test proves nothing.
        for hero in [0, 3] {
            for hand in ["main_hand", "off_hand"] {
                let starter = db
                    .get_gear(p)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|g| g.equipped_hero_slot == Some(hero) && g.slot == hand)
                    .expect("starter hand slot");
                assert_eq!(
                    db.set_equipped(p, starter.gear_id, None).await.unwrap(),
                    EquipResult::Ok
                );
            }
        }
        let base = db.equipped_gear_bonuses(p, 4, &classes).await.unwrap();

        let spear = LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::now_v7(),
            name: "Warpike".into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            tier: 3,
            atk_bonus: 9,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 80,
            max_durability: 80,
            damage_modifiers: String::new(),
            family: "spear".into(),
            armor_weight: String::new(),
            affixes: "[]".into(),
        unique_key: String::new(),
        set_key: String::new(),
        };
        let spear_id = spear.gear_id;
        db.insert_looted_gear(p, &[spear]).await.unwrap();

        // On the Resonant the spear equips (equip-time legality needs a persisted
        // hero class — GR-7) but grants NOTHING: derivation is the authority.
        assert_eq!(db.set_equipped(p, spear_id, Some(0)).await.unwrap(), EquipResult::Ok);
        let with_resonant = db.equipped_gear_bonuses(p, 4, &classes).await.unwrap();
        assert_eq!(
            with_resonant[0].atk, base[0].atk,
            "a Resonant gains nothing from a spear"
        );

        // The same piece on an Explorer does apply.
        assert_eq!(db.set_equipped(p, spear_id, None).await.unwrap(), EquipResult::Ok);
        assert_eq!(db.set_equipped(p, spear_id, Some(3)).await.unwrap(), EquipResult::Ok);
        let with_explorer = db.equipped_gear_bonuses(p, 4, &classes).await.unwrap();
        assert_eq!(
            with_explorer[3].atk,
            base[3].atk + 9,
            "an Explorer may carry the spear"
        );
    }

    #[tokio::test]
    async fn equip_is_refused_with_the_reason_once_a_hero_has_a_class() {
        use meld_proto::equipment::Legality;
        let db = mem().await;
        let p = db.register("gr7", "pw").await.unwrap().player_id;
        // Hero 0 is a Resonant (staff, robes); hero 1 an Explorer.
        db.set_hero_class(p, 0, "resonant").await.unwrap();
        db.set_hero_class(p, 1, "explorer").await.unwrap();
        assert_eq!(
            db.get_hero_classes(p).await.unwrap()[..2],
            ["resonant".to_string(), "explorer".to_string()]
        );

        let piece = |name: &str, slot: &str, family: &str, weight: &str| LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::now_v7(),
            name: name.into(),
            slot: slot.into(),
            class_key: String::new(),
            tier: 2,
            atk_bonus: 5,
            def_bonus: 5,
            spd_bonus: 0,
            base_max_durability: 80,
            max_durability: 80,
            damage_modifiers: String::new(),
            family: family.into(),
            armor_weight: weight.into(),
            affixes: "[]".into(),
        unique_key: String::new(),
        set_key: String::new(),
        };
        let spear = piece("Warpike", "main_hand", "spear", "");
        let plate = piece("Battleplate", "chest", "", "heavy");
        let staff = piece("Ward Stave", "main_hand", "staff", "");
        let shield = piece("Targe", "off_hand", "shield", "");
        let (spear_id, plate_id, staff_id, shield_id) =
            (spear.gear_id, plate.gear_id, staff.gear_id, shield.gear_id);
        db.insert_looted_gear(p, &[spear, plate, staff, shield]).await.unwrap();
        // Free both heroes' hands of the starter kit first.
        for g in db.get_gear(p).await.unwrap() {
            if g.equipped_hero_slot.is_some() && (g.slot == "main_hand" || g.slot == "off_hand" || g.slot == "chest") {
                db.set_equipped(p, g.gear_id, None).await.unwrap();
            }
        }

        // The Resonant is refused the spear and the plate, each naming its rule.
        assert_eq!(
            db.set_equipped(p, spear_id, Some(0)).await.unwrap(),
            EquipResult::ClassLocked(Legality::ClassFamily)
        );
        assert_eq!(
            db.set_equipped(p, plate_id, Some(0)).await.unwrap(),
            EquipResult::ClassLocked(Legality::ClassWeight)
        );
        // Its own staff is fine.
        assert_eq!(db.set_equipped(p, staff_id, Some(0)).await.unwrap(), EquipResult::Ok);
        // A shield on the Resonant is refused by the CLASS rule, which is checked
        // before the hands rule — the more specific answer wins.
        assert_eq!(
            db.set_equipped(p, shield_id, Some(0)).await.unwrap(),
            EquipResult::ClassLocked(Legality::ClassFamily)
        );
        // The Explorer may hold either, but not both: sword+shield OR the spear.
        assert_eq!(db.set_equipped(p, shield_id, Some(1)).await.unwrap(), EquipResult::Ok);
        assert_eq!(
            db.set_equipped(p, spear_id, Some(1)).await.unwrap(),
            EquipResult::TwoHandedConflict
        );
        assert_eq!(db.set_equipped(p, shield_id, None).await.unwrap(), EquipResult::Ok);
        assert_eq!(db.set_equipped(p, spear_id, Some(1)).await.unwrap(), EquipResult::Ok);
    }

    #[tokio::test]
    async fn a_hero_with_no_recorded_class_is_never_locked_out() {
        let db = mem().await;
        let p = db.register("gr7b", "pw").await.unwrap().player_id;
        let staff = LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::now_v7(),
            name: "Ward Stave".into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            tier: 2,
            atk_bonus: 5,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 80,
            max_durability: 80,
            damage_modifiers: String::new(),
            family: "staff".into(),
            armor_weight: String::new(),
            affixes: "[]".into(),
        unique_key: String::new(),
        set_key: String::new(),
        };
        let staff_id = staff.gear_id;
        db.insert_looted_gear(p, &[staff]).await.unwrap();
        for g in db.get_gear(p).await.unwrap() {
            if g.equipped_hero_slot == Some(0) && (g.slot == "main_hand" || g.slot == "off_hand") {
                db.set_equipped(p, g.gear_id, None).await.unwrap();
            }
        }
        // No class recorded for hero 0 → allowed; derivation is the backstop, so a
        // player is never locked out of their own Vault by missing data.
        assert_eq!(db.set_equipped(p, staff_id, Some(0)).await.unwrap(), EquipResult::Ok);
    }

    #[tokio::test]
    async fn affixes_fold_by_kind_and_respect_the_wearer_s_class() {
        use meld_proto::affixes::Affix;
        let db = mem().await;
        let test_password = Uuid::now_v7().to_string();
        let p = db.register("ad1", &test_password).await.unwrap().player_id;
        db.set_hero_class(p, 0, "explorer").await.unwrap();
        db.set_hero_class(p, 1, "psyker").await.unwrap();
        let aff = |key: &str, m: i32| Affix {
            key: key.into(),
            magnitude: m,
            element: None,
            ally_class: None,
        };
        let rolled = vec![
            aff("atk", 4),
            aff("barrier", 12),
            aff("regen", 3),
            aff("evasion", 7),
            aff("adrenaline_primed", 5),
            aff("focus_slot", 1),
            Affix { key: "resist".into(), magnitude: 25, element: Some("FIRE".into()), ally_class: None },
            Affix { key: "ally_atk".into(), magnitude: 6, element: None, ally_class: Some("resonant".into()) },
        ];
        let piece = LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::now_v7(),
            name: "Warblade of the Bulwark".into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            tier: 9,
            atk_bonus: 3,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 80,
            max_durability: 80,
            damage_modifiers: String::new(),
            family: "sword".into(),
            armor_weight: String::new(),
            affixes: meld_proto::affixes::to_json(&rolled),
        unique_key: String::new(),
        set_key: String::new(),
        };
        let gid = piece.gear_id;
        db.insert_looted_gear(p, &[piece]).await.unwrap();
        for g in db.get_gear(p).await.unwrap() {
            if g.equipped_hero_slot == Some(0) && g.slot == "main_hand" {
                db.set_equipped(p, g.gear_id, None).await.unwrap();
            }
        }
        db.set_equipped(p, gid, Some(0)).await.unwrap();

        let classes: Vec<String> = ["explorer", "psyker", "resonant", "explorer"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let b = &db.equipped_gear_bonuses(p, 4, &classes).await.unwrap()[0];
        // Stat + ward affixes apply to anyone.
        assert!(b.atk >= 4 + 3, "stat affix + base atk: {}", b.atk);
        assert_eq!(b.barrier, 12);
        assert_eq!(b.regen, 3);
        assert_eq!(b.evasion, 7);
        // The Explorer keyword lands; the Psyker one does not (wrong wearer).
        assert_eq!(b.adrenaline, 5);
        assert_eq!(b.focus_slots, 0, "a Psyker affix is inert on an Explorer");
        // A resist affix becomes a damage multiplier (25% resisted -> 0.75).
        assert!(b.modifiers.iter().any(|(el, m)| el == "FIRE" && (*m - 0.75).abs() < 1e-9));
        // Synergy is deferred to battle assembly, which knows the party.
        assert_eq!(b.synergies, vec![("resonant".to_string(), 6, 0)]);
    }

    #[tokio::test]
    async fn the_apothecary_takes_chits_and_never_bills_for_nothing() {
        let db = mem().await;
        let p = db.register("shopper", "pw").await.unwrap().player_id;
        // Bank some chits the way an extraction would.
        db.bank_extraction(p, &[], 100).await.unwrap();
        assert_eq!(db.get_vault(p).await.unwrap().0, 100);

        // Two salves at 25 each.
        assert!(db.buy_from_vendor(p, "bloom_salve", 2, 25).await.unwrap());
        assert_eq!(db.get_vault(p).await.unwrap().0, 50);
        let (_, items) = db.get_vault(p).await.unwrap();
        assert_eq!(
            items.iter().find(|(k, _)| k == "bloom_salve").map(|(_, q)| *q),
            Some(2)
        );

        // Too expensive: refused, and NOTHING moves — not the chits, not the goods.
        assert!(!db.buy_from_vendor(p, "elixir", 10, 999).await.unwrap());
        assert_eq!(
            db.get_vault(p).await.unwrap().0,
            50,
            "a failed purchase billed the player"
        );
        let (_, items) = db.get_vault(p).await.unwrap();
        assert!(items.iter().all(|(k, _)| k != "elixir"), "goods arrived unpaid");

        // Nonsense quantities are refused rather than interpreted.
        assert!(!db.buy_from_vendor(p, "bloom_salve", 0, 25).await.unwrap());
        assert!(!db.buy_from_vendor(p, "bloom_salve", -3, 25).await.unwrap());
        assert_eq!(db.get_vault(p).await.unwrap().0, 50);
    }

    #[tokio::test]
    async fn a_loadout_can_never_equip_gear_the_account_does_not_own() {
        // The anti-cheat property PT-2 rests on. A loadout is applied by NAME and the
        // server replays the gear ids IT captured — but those ids are replayed through
        // `set_equipped`, which scopes every lookup to the owner. So even a loadout
        // holding a stranger's id (a hand-edited row, a copied database) equips
        // nothing: the item is simply not found for this player.
        let db = mem().await;
        let mine = db.register("lo_mine", "correct-horse-battery").await.unwrap().player_id;
        let theirs = db.register("lo_theirs", "correct-horse-battery").await.unwrap().player_id;

        let blade = LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::new_v4(),
            name: "Their Blade".into(),
            slot: "main_hand".into(),
            class_key: String::new(),
            tier: 1,
            atk_bonus: 5,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 80,
            max_durability: 80,
            damage_modifiers: "{}".into(),
            family: String::new(),
            armor_weight: String::new(),
            affixes: "[]".into(),
            unique_key: String::new(),
            set_key: String::new(),
        };
        let stolen_id = blade.gear_id;
        db.insert_looted_gear(theirs, &[blade]).await.unwrap();

        // Forge a loadout naming somebody else's item.
        db.save_loadout(mine, "Cheat", &["explorer".to_string()], &[(0, stolen_id)])
            .await
            .unwrap();
        let saved = db.list_loadouts(mine).await.unwrap();
        assert_eq!(saved[0].gear, vec![(0, stolen_id)], "the row stores what it was given");

        // Replaying it equips nothing: the owner scope refuses.
        assert_eq!(
            db.set_equipped(mine, stolen_id, Some(0)).await.unwrap(),
            EquipResult::NotFound,
            "another account's gear must not be equippable"
        );
        // And it is still on its real owner, untouched (alongside their starter kit).
        assert!(
            db.get_gear(theirs).await.unwrap().iter().any(|g| g.gear_id == stolen_id),
            "the item should still belong to the account that looted it"
        );
        assert!(
            db.get_gear(mine).await.unwrap().iter().all(|g| g.gear_id != stolen_id),
            "it must not have crossed accounts"
        );

        // A gear id that has ceased to exist is skipped the same way, which is the
        // "it got wrecked or sold since you saved" case.
        assert_eq!(
            db.set_equipped(mine, Uuid::new_v4(), Some(0)).await.unwrap(),
            EquipResult::NotFound
        );
    }

    #[tokio::test]
    async fn a_potion_craft_credits_alchemy_and_a_forge_craft_credits_forging() {
        let db = mem().await;
        let p = db.register("brewer", "pw").await.unwrap().player_id;
        db.bank_extraction(
            p,
            &[("bloom_herb".into(), 4), ("dune_iron".into(), 1), ("sun_salts".into(), 1)],
            0,
        )
        .await
        .unwrap();
        async fn skill_xp(db: &Db, p: Uuid, want: &str) -> i64 {
            db.get_skills(p)
                .await
                .unwrap()
                .into_iter()
                .find(|(k, _)| k == want)
                .map(|(_, xp)| xp)
                .unwrap_or(0)
        }

        // A potion is Alchemy's business.
        let r = meld_proto::consumables::recipe("bloom_salve").unwrap();
        let inputs: Vec<(String, i32)> =
            r.inputs.iter().map(|(k, q)| ((*k).to_string(), *q)).collect();
        assert!(db
            .craft(p, &inputs, (r.output, r.output_qty), r.skill, 10)
            .await
            .unwrap());
        assert_eq!(skill_xp(&db, p, "alchemy").await, 10);
        assert_eq!(skill_xp(&db, p, "forging").await, 0, "a potion credited Forging");

        // Metalwork is Forging's.
        let r = meld_proto::consumables::recipe("town_portal").unwrap();
        let inputs: Vec<(String, i32)> =
            r.inputs.iter().map(|(k, q)| ((*k).to_string(), *q)).collect();
        assert!(db
            .craft(p, &inputs, (r.output, r.output_qty), r.skill, 10)
            .await
            .unwrap());
        assert_eq!(skill_xp(&db, p, "forging").await, 10);
        assert_eq!(skill_xp(&db, p, "alchemy").await, 10, "forging leaked into alchemy");
    }

    #[tokio::test]
    async fn the_forge_charges_atomically_and_repair_never_overshoots() {
        let db = mem().await;
        let p = db.register("smith", "pw").await.unwrap().player_id;
        db.bank_extraction(p, &[("dune_iron".into(), 10)], 500).await.unwrap();
        let piece = LootedGear {
            insurance: meld_proto::Insurance::Standard,
            gear_id: Uuid::now_v7(),
            name: "Forged Warblade".into(),
            slot: "main_hand".into(),
            class_key: "explorer".into(),
            tier: 3,
            atk_bonus: 9,
            def_bonus: 0,
            spd_bonus: 0,
            base_max_durability: 100,
            max_durability: 100,
            damage_modifiers: "{}".into(),
            family: "sword".into(),
            armor_weight: String::new(),
            affixes: "[]".into(),
            unique_key: String::new(),
            set_key: String::new(),
        };
        let gid = piece.gear_id;

        // Too expensive: nothing moves — not the chits, not the materials, no gear.
        assert!(!db
            .forge_gear(p, &[("dune_iron".into(), 4)], 99_999, &piece)
            .await
            .unwrap());
        let (chits, items) = db.get_vault(p).await.unwrap();
        assert_eq!(chits, 500, "a failed forge billed the smith");
        assert_eq!(items.iter().find(|(k, _)| k == "dune_iron").map(|(_, q)| *q), Some(10));
        assert!(db.get_gear_by_id(p, gid).await.unwrap().is_none());

        // Affordable: materials and chits leave, the piece arrives, and it is INSURED
        // (a smith's own work survives a death).
        assert!(db
            .forge_gear(p, &[("dune_iron".into(), 4)], 60, &piece)
            .await
            .unwrap());
        let (chits, items) = db.get_vault(p).await.unwrap();
        assert_eq!(chits, 440);
        assert_eq!(items.iter().find(|(k, _)| k == "dune_iron").map(|(_, q)| *q), Some(6));
        let row = db.get_gear_by_id(p, gid).await.unwrap().expect("forged piece");
        assert_eq!(row.insurance, "blue");
        assert_eq!(row.family, "sword");

        // A reroll swaps the affixes for a price and leaves the STATS alone.
        assert!(db
            .reroll_gear_affixes(p, gid, &[("dune_iron".into(), 3)], 90, "[{\"key\":\"barrier\",\"magnitude\":11}]")
            .await
            .unwrap());
        let row = db.get_gear_by_id(p, gid).await.unwrap().unwrap();
        assert!(row.affixes.contains("barrier"));
        assert_eq!(row.atk_bonus, 9, "a reroll changed the stats");
        assert_eq!(db.get_vault(p).await.unwrap().0, 350);

        // Repair: chew the durability the way a death does, then buy it back. It
        // never exceeds the piece's original maximum, and it only bills for what it
        // actually restored.
        // Only EQUIPPED insured gear takes the death penalty, and hero 0's hand is
        // full of starter kit — free it, or the equip silently no-ops.
        let starter = db
            .get_gear(p)
            .await
            .unwrap()
            .into_iter()
            .find(|g| g.equipped_hero_slot == Some(0) && g.slot == "main_hand")
            .expect("starter main-hand");
        assert_eq!(db.set_equipped(p, starter.gear_id, None).await.unwrap(), EquipResult::Ok);
        assert_eq!(db.set_equipped(p, gid, Some(0)).await.unwrap(), EquipResult::Ok);
        db.apply_death_durability(p, 0.1).await.unwrap();
        let chewed = db.get_gear_by_id(p, gid).await.unwrap().unwrap();
        assert!(chewed.max_durability < chewed.base_max_durability, "nothing was chewed");
        let missing = chewed.base_max_durability - chewed.max_durability;

        let before = db.get_vault(p).await.unwrap().0;
        let restored = db.repair_gear(p, gid, 9_999, 4).await.unwrap();
        assert_eq!(restored, missing, "repair restored {restored} of {missing}");
        let row = db.get_gear_by_id(p, gid).await.unwrap().unwrap();
        assert_eq!(row.max_durability, row.base_max_durability);
        assert_eq!(db.get_vault(p).await.unwrap().0, before - restored as i64 * 4);

        // Nothing left to repair → nothing charged.
        assert_eq!(db.repair_gear(p, gid, 50, 4).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_class_best_only_ever_climbs() {
        let db = mem().await;
        let p = db.register("recorder", "pw").await.unwrap().player_id;
        assert!(db.get_class_bests(p).await.unwrap().is_empty());

        // A new record sticks…
        assert!(db.record_class_best(p, "explorer", 12).await.unwrap());
        // …a shallower dive never lowers it (XP is dive-scoped; the ACHIEVEMENT is not).
        assert!(!db.record_class_best(p, "explorer", 5).await.unwrap());
        assert!(db.record_class_best(p, "explorer", 31).await.unwrap());

        // Records are per class, and read back deepest first.
        assert!(db.record_class_best(p, "resonant", 20).await.unwrap());
        let bests = db.get_class_bests(p).await.unwrap();
        assert_eq!(bests[0], ("explorer".to_string(), 31));
        assert_eq!(bests[1], ("resonant".to_string(), 20));

        // Nonsense is refused rather than stored.
        assert!(!db.record_class_best(p, "explorer", 0).await.unwrap());
        assert!(!db.record_class_best(p, "", 40).await.unwrap());
        assert_eq!(db.get_class_bests(p).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unlocks_are_permanent_idempotent_and_start_with_the_explorer() {
        let db = mem().await;
        let p = db.register("orla", "pw").await.unwrap().player_id;
        // A brand-new account owns the starting set even with no rows written —
        // an account made before unlocks existed must not be locked out.
        assert_eq!(db.get_unlocks(p).await.unwrap(), vec!["class_explorer".to_string()]);

        let granted = db
            .grant_unlocks(p, &["party_slot_2".to_string(), "class_resonant".to_string()])
            .await
            .unwrap();
        assert_eq!(granted, vec!["party_slot_2", "class_resonant"]);
        // Re-granting the same milestone gives nothing, so the loop can fire it
        // freely without remembering whether it already did.
        assert!(db
            .grant_unlocks(p, &["party_slot_2".to_string(), "class_resonant".to_string()])
            .await
            .unwrap()
            .is_empty());
        let owned = db.get_unlocks(p).await.unwrap();
        assert_eq!(owned, vec!["class_explorer", "class_resonant", "party_slot_2"]);
        assert_eq!(meld_proto::unlocks::party_slots(&owned), 2);

        // And they are per account: another player's night is not yours.
        let q = db.register("bel", "pw").await.unwrap().player_id;
        assert_eq!(db.get_unlocks(q).await.unwrap(), vec!["class_explorer".to_string()]);
    }
}

// ------------------------------------------------------- the Vanguard Board ---

/// One in-memory Vanguard posting. A named struct rather than a tuple because it grew a
/// route (level / fights / flees) and a five-tuple stops saying what its fields are.
#[derive(Debug, Clone, Copy)]
struct MemVanguard {
    distance: i32,
    at: DateTime<Utc>,
    at_level: i32,
    fights: i32,
    flees: i32,
    star: bool,
    clear_ms: Option<i64>,
}

/// One stored Vanguard Board record (roadmap P1-1). Unranked — rank is assigned
/// by the reader from the query's order, so a slice of the board still ranks 1..n.
#[derive(Debug, Clone)]
pub struct VanguardRow {
    pub player_id: Uuid,
    pub username: String,
    pub max_distance: i32,
    pub achieved_at: DateTime<Utc>,
    /// How the run got there — see `record_vanguard_distance`.
    pub at_level: i32,
    pub fights: i32,
    pub flees: i32,
    pub star: Option<String>,
    pub clear_ms: Option<i64>,
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

/// Insert one owned, unequipped piece of gear inside an open transaction.
///
/// The single Postgres write behind every piece the persistent world hands over —
/// extraction banking and a Hunt Board payout — so the column list cannot drift between
/// them.
async fn insert_gear_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    player_id: Uuid,
    g: &LootedGear,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO gear (gear_id, owner_player_id, name, slot, class_key, insurance, tier, atk_bonus, def_bonus, spd_bonus, base_max_durability, max_durability, equipped_hero_slot, damage_modifiers, family, armor_weight, affixes, unique_key, set_key)
         VALUES ($1, $2, $3, $4, $5, $18, $6, $7, $8, $9, $10, $11, NULL, $12, $13, $14, $15, $16, $17)
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
    .bind(&g.family)
    .bind(&g.armor_weight)
    .bind(&g.affixes)
    .bind(&g.unique_key)
    .bind(&g.set_key)
    .bind(insurance_word(g.insurance))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// The in-memory backend's mirror of [`insert_gear_row`].
fn mem_gear_row(player_id: Uuid, g: &LootedGear) -> MemGear {
    MemGear {
        gear_id: g.gear_id,
        owner_player_id: player_id,
        name: g.name.clone(),
        slot: g.slot.clone(),
        class_key: g.class_key.clone(),
        insurance: insurance_word(g.insurance).to_string(),
        family: g.family.clone(),
        armor_weight: g.armor_weight.clone(),
        affixes: g.affixes.clone(),
        unique_key: g.unique_key.clone(),
        set_key: g.set_key.clone(),
        tier: g.tier,
        atk_bonus: g.atk_bonus,
        def_bonus: g.def_bonus,
        spd_bonus: g.spd_bonus,
        base_max_durability: g.base_max_durability,
        max_durability: g.max_durability,
        equipped_hero_slot: None,
        damage_modifiers: g.damage_modifiers.clone(),
    }
}

// ----------------------------------------------------------- the Hunt Board ---

/// One stored hunt record (roadmap AD-4). Absent from the table means untouched:
/// zero progress, unclaimed.
#[derive(Debug, Clone)]
pub struct HuntRow {
    pub hunt_key: String,
    pub progress: i32,
    pub claimed: bool,
}

/// What crediting an event did to one hunt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HuntCredit {
    pub progress: i32,
    /// This credit is the one that finished it — true exactly once per hunt.
    pub completed: bool,
}

/// The board's answer to a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HuntClaim {
    Paid { chits: i64 },
    NotEarned { progress: i32 },
    AlreadyClaimed,
}

/// One stored bounty contract (roadmap AD-4). `spec` is a serialized
/// `meld_proto::bounties::BountySpec` — rolled once, then owned by the row.
#[derive(Debug, Clone)]
pub struct BountyRow {
    pub bounty_id: Uuid,
    pub spec: String,
    pub state: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// The Den's answer to a bounty claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BountyClaim {
    Paid { chits: i64 },
    /// The mark is still standing.
    NotCompleted,
    AlreadyClaimed,
    /// No such contract, or not this player's.
    Missing,
}
