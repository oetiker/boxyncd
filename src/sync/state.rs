use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

/// A row from the sync_entries table — the baseline for three-way comparison.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SyncEntry {
    pub id: i64,
    pub sync_root_index: i64,
    pub box_id: Option<String>,
    pub box_parent_id: Option<String>,
    pub entry_type: String,
    pub relative_path: String,
    pub local_sha1: Option<String>,
    pub remote_sha1: Option<String>,
    pub remote_etag: Option<String>,
    pub remote_version_id: Option<String>,
    pub remote_modified_at: Option<String>,
    pub local_modified_at: Option<String>,
    pub local_size: Option<i64>,
    pub sync_status: String,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub retry_count: i64,
}

impl SyncEntry {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Self {
        Self {
            id: row.get("id"),
            sync_root_index: row.get("sync_root_index"),
            box_id: row.get("box_id"),
            box_parent_id: row.get("box_parent_id"),
            entry_type: row.get("entry_type"),
            relative_path: row.get("relative_path"),
            local_sha1: row.get("local_sha1"),
            remote_sha1: row.get("remote_sha1"),
            remote_etag: row.get("remote_etag"),
            remote_version_id: row.get("remote_version_id"),
            remote_modified_at: row.get("remote_modified_at"),
            local_modified_at: row.get("local_modified_at"),
            local_size: row.get("local_size"),
            sync_status: row.get("sync_status"),
            last_sync_at: row.get("last_sync_at"),
            last_error: row.get("last_error"),
            retry_count: row.get("retry_count"),
        }
    }
}

/// Get all sync entries for a given sync root.
pub async fn get_all_for_root(pool: &SqlitePool, root_index: i64) -> Result<Vec<SyncEntry>> {
    let rows = sqlx::query("SELECT * FROM sync_entries WHERE sync_root_index = ?")
        .bind(root_index)
        .fetch_all(pool)
        .await
        .context("Failed to fetch sync entries")?;

    Ok(rows.iter().map(SyncEntry::from_row).collect())
}

/// Look up a sync entry by (root_index, relative_path).
pub async fn get_by_path(
    pool: &SqlitePool,
    root_index: i64,
    relative_path: &str,
) -> Result<Option<SyncEntry>> {
    let row =
        sqlx::query("SELECT * FROM sync_entries WHERE sync_root_index = ? AND relative_path = ?")
            .bind(root_index)
            .bind(relative_path)
            .fetch_optional(pool)
            .await
            .context("Failed to fetch sync entry by path")?;

    Ok(row.as_ref().map(SyncEntry::from_row))
}

/// Insert or update a sync entry (upsert on sync_root_index + relative_path).
pub async fn upsert(pool: &SqlitePool, e: &SyncEntry) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();

    let result = sqlx::query(
        r#"INSERT INTO sync_entries (
            sync_root_index, box_id, box_parent_id, entry_type, relative_path,
            local_sha1, remote_sha1, remote_etag, remote_version_id,
            remote_modified_at, local_modified_at, local_size,
            sync_status, last_sync_at, last_error, retry_count
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(sync_root_index, relative_path) DO UPDATE SET
            box_id = excluded.box_id,
            box_parent_id = excluded.box_parent_id,
            entry_type = excluded.entry_type,
            local_sha1 = excluded.local_sha1,
            remote_sha1 = excluded.remote_sha1,
            remote_etag = excluded.remote_etag,
            remote_version_id = excluded.remote_version_id,
            remote_modified_at = excluded.remote_modified_at,
            local_modified_at = excluded.local_modified_at,
            local_size = excluded.local_size,
            sync_status = excluded.sync_status,
            last_sync_at = excluded.last_sync_at,
            last_error = excluded.last_error,
            retry_count = excluded.retry_count"#,
    )
    .bind(e.sync_root_index)
    .bind(&e.box_id)
    .bind(&e.box_parent_id)
    .bind(&e.entry_type)
    .bind(&e.relative_path)
    .bind(&e.local_sha1)
    .bind(&e.remote_sha1)
    .bind(&e.remote_etag)
    .bind(&e.remote_version_id)
    .bind(&e.remote_modified_at)
    .bind(&e.local_modified_at)
    .bind(e.local_size)
    .bind(&e.sync_status)
    .bind(&now)
    .bind(&e.last_error)
    .bind(e.retry_count)
    .execute(pool)
    .await
    .context("Failed to upsert sync entry")?;

    Ok(result.last_insert_rowid())
}

/// Delete a sync entry by ID.
pub async fn delete_entry(pool: &SqlitePool, entry_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM sync_entries WHERE id = ?")
        .bind(entry_id)
        .execute(pool)
        .await
        .context("Failed to delete sync entry")?;
    Ok(())
}

/// Mark an entry as errored with a message.
pub async fn set_error(pool: &SqlitePool, entry_id: i64, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE sync_entries SET sync_status = 'error', last_error = ?, retry_count = retry_count + 1 WHERE id = ?",
    )
    .bind(error)
    .bind(entry_id)
    .execute(pool)
    .await
    .context("Failed to update error status")?;
    Ok(())
}

/// Look up a sync entry by its Box ID (any root).
pub async fn get_by_box_id_any_root(pool: &SqlitePool, box_id: &str) -> Result<Option<SyncEntry>> {
    let row = sqlx::query("SELECT * FROM sync_entries WHERE box_id = ? LIMIT 1")
        .bind(box_id)
        .fetch_optional(pool)
        .await
        .context("Failed to fetch sync entry by box_id")?;

    Ok(row.as_ref().map(SyncEntry::from_row))
}

/// Get stream position for a sync root.
pub async fn get_stream_position(pool: &SqlitePool, root_index: i64) -> Result<Option<String>> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT stream_position FROM stream_positions WHERE sync_root_index = ?")
            .bind(root_index)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Look up a sync entry by its Box ID and root index.
pub async fn get_by_box_id(
    pool: &SqlitePool,
    root_index: i64,
    box_id: &str,
) -> Result<Option<SyncEntry>> {
    let row = sqlx::query("SELECT * FROM sync_entries WHERE sync_root_index = ? AND box_id = ?")
        .bind(root_index)
        .bind(box_id)
        .fetch_optional(pool)
        .await
        .context("Failed to fetch sync entry by box_id")?;

    Ok(row.as_ref().map(SyncEntry::from_row))
}

/// Find which sync root owns an item by its Box ID.
pub async fn find_root_by_box_id(pool: &SqlitePool, box_id: &str) -> Result<Option<i64>> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT sync_root_index FROM sync_entries WHERE box_id = ? LIMIT 1")
            .bind(box_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Set stream position for a sync root.
pub async fn set_stream_position(pool: &SqlitePool, root_index: i64, position: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"INSERT INTO stream_positions (sync_root_index, stream_position, updated_at)
        VALUES (?, ?, ?)
        ON CONFLICT(sync_root_index) DO UPDATE SET
            stream_position = excluded.stream_position,
            updated_at = excluded.updated_at"#,
    )
    .bind(root_index)
    .bind(position)
    .bind(&now)
    .execute(pool)
    .await
    .context("Failed to update stream position")?;
    Ok(())
}
