use anyhow::{Context, Result};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

pub async fn init_db() -> Result<SqlitePool> {
    let data_dir = dirs::data_dir()
        .context("Could not determine data directory")?
        .join("boxyncd");
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;

    let db_path = data_dir.join("sync.db");
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
