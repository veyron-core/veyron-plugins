use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::str::FromStr;

pub struct DbConfig {
    pub data_dir: PathBuf,
    pub pool_size: u32,
    pub busy_timeout_ms: u64,
    /// Hard disk ceiling for the store, enforced by SQLite itself via
    /// `PRAGMA max_page_count` (writes past it fail with `SQLITE_FULL`).
    /// This bounds the whole database file regardless of which action wrote
    /// to it — `max_value_bytes` only guards the `sync_set` fast path.
    /// `0` disables the quota.
    pub max_db_bytes: u64,
}

/// Fixed SQLite page size, set explicitly so the byte→page arithmetic for
/// the `max_page_count` quota is exact rather than depending on the
/// compiled-in default.
const PAGE_SIZE: u32 = 4096;

/// Single shared store (not per-caller like `database`): sync is the host's
/// one global versioned KV, so there is exactly one file — `sync.db` — under
/// `data_dir`, opened once and shared by every handler task via the pool.
const DB_FILE: &str = "sync.db";

const STATE_TABLE_DDL: &str = "create table if not exists state (\
    key TEXT PRIMARY KEY, \
    value TEXT NOT NULL, \
    updated_at INTEGER NOT NULL\
)";

const META_TABLE_DDL: &str = "create table if not exists meta (\
    key TEXT PRIMARY KEY, \
    value TEXT NOT NULL\
)";

/// Open the single shared pool, applying WAL + busy timeout + the optional
/// disk quota, then create both tables and seed the version counter. `meta`
/// holds the persisted monotonic version under key `"version"` (TEXT, so a
/// restarted plugin resumes where it left off instead of re-zeroing).
pub async fn open_pool(config: &DbConfig) -> Result<SqlitePool, String> {
    std::fs::create_dir_all(&config.data_dir)
        .map_err(|e| format!("failed to create data_dir: {e}"))?;
    let db_path = config.data_dir.join(DB_FILE);

    let mut options = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))
        .map_err(|e| format!("invalid sqlite path: {e}"))?
        .create_if_missing(true)
        .busy_timeout(std::time::Duration::from_millis(config.busy_timeout_ms))
        .page_size(PAGE_SIZE)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

    // Hard disk quota. `PRAGMA max_page_count` is enforced by SQLite itself,
    // so a write that would grow the file past the ceiling fails with
    // SQLITE_FULL. sqlx replays these pragmas on every pooled connection
    // (SqliteConnectOptions::pragma_string), so the ceiling holds across the
    // whole pool. `0` disables it.
    if config.max_db_bytes > 0 {
        let max_pages = (config.max_db_bytes / PAGE_SIZE as u64).max(1);
        options = options.pragma("max_page_count", max_pages.to_string());
    }

    let pool = SqlitePoolOptions::new()
        .max_connections(config.pool_size)
        .connect_with(options)
        .await
        .map_err(|e| format!("failed to open database: {e}"))?;

    sqlx::query(STATE_TABLE_DDL)
        .execute(&pool)
        .await
        .map_err(|e| format!("failed to init state table: {e}"))?;
    sqlx::query(META_TABLE_DDL)
        .execute(&pool)
        .await
        .map_err(|e| format!("failed to init meta table: {e}"))?;

    // Seed the version counter once. `on conflict do nothing` keeps a prior
    // value on reopen — a plugin restart must not reset the store's version.
    sqlx::query(
        "insert into meta (key, value) values ('version', '0') on conflict(key) do nothing",
    )
    .execute(&pool)
    .await
    .map_err(|e| format!("failed to init version: {e}"))?;

    Ok(pool)
}
