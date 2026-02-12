use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

/// Resolve the database file path: use the custom path if provided,
/// otherwise fall back to `$XDG_DATA_HOME/boxyncd/sync.db`.
pub fn resolve_db_path(custom: Option<&Path>) -> Result<PathBuf> {
    match custom {
        Some(p) => Ok(p.to_path_buf()),
        None => {
            let dir = dirs::data_dir().context("Could not determine data directory")?;
            Ok(dir.join("boxyncd").join("sync.db"))
        }
    }
}

/// Open the existing database in read-only mode.
/// Returns `None` if the database file doesn't exist (first run).
/// Skips migrations since we can't write.
pub async fn open_db_readonly(custom: Option<&Path>) -> Result<Option<SqlitePool>> {
    let db_path = resolve_db_path(custom)?;

    if !db_path.exists() {
        return Ok(None);
    }

    let db_url = format!("sqlite:{}?mode=ro", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .with_context(|| format!("Failed to open database read-only: {}", db_path.display()))?;

    Ok(Some(pool))
}

pub async fn init_db(custom: Option<&Path>) -> Result<SqlitePool> {
    let db_path = resolve_db_path(custom)?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create data directory: {}", parent.display()))?;
    }

    let db_url = format!("sqlite:{}?mode=rwc", db_path.display());

    tracing::debug!(path = %db_path.display(), "opening database");

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await
        .with_context(|| format!("Failed to open database: {}", db_path.display()))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("Failed to run database migrations")?;

    tracing::info!(path = %db_path.display(), "database initialized");
    Ok(pool)
}
