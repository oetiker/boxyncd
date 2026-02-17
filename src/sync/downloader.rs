use std::path::Path;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use crate::box_api::BoxClient;
use crate::util::hash;

use super::state::{self, SyncEntry};

/// Download a file from Box and record the synced state in the DB.
///
/// Writes to a `.boxyncd.tmp` file first, then renames for atomicity.
/// After writing, computes the local SHA-1 and upserts the DB entry.
pub async fn download_and_record(
    client: &BoxClient,
    pool: &SqlitePool,
    box_id: &str,
    relative_path: &str,
    base_path: &Path,
    root_index: i64,
) -> Result<()> {
    let dest = base_path.join(relative_path);

    // Ensure parent directory exists
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create parent dir: {}", parent.display()))?;
    }

    // Download (atomic via BoxClient — writes .boxyncd.tmp then renames)
    client
        .download_file(box_id, &dest)
        .await
        .with_context(|| format!("Failed to download {relative_path}"))?;

    // Get the remote file metadata for DB recording
    let remote_file = client
        .get_file(box_id)
        .await
        .with_context(|| format!("Failed to get metadata for {box_id}"))?;

    // Compute local SHA-1
    let local_sha1 = hash::compute_sha1(&dest).await?;

    // Get local file metadata for mtime/size
    let local_meta = tokio::fs::metadata(&dest).await?;
    let local_mtime = {
        use std::time::UNIX_EPOCH;
        let dur = local_meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        chrono::DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
            .unwrap_or_default()
            .to_rfc3339()
    };

    let entry = SyncEntry {
        id: 0, // ignored by upsert
        sync_root_index: root_index,
        box_id: Some(remote_file.id),
        box_parent_id: remote_file.parent.as_ref().map(|p| p.id.clone()),
        entry_type: "file".into(),
        relative_path: relative_path.to_string(),
        local_sha1: Some(local_sha1),
        remote_sha1: remote_file.sha1,
        remote_etag: remote_file.etag,
        remote_version_id: remote_file.file_version.as_ref().map(|v| v.id.clone()),
        remote_modified_at: remote_file.content_modified_at.or(remote_file.modified_at),
        local_modified_at: Some(local_mtime),
        local_size: Some(local_meta.len() as i64),
        sync_status: "synced".into(),
        last_sync_at: None, // set by upsert
        last_error: None,
        retry_count: 0,
        trashed_at: None,
        trash_path: None,
    };

    state::upsert(pool, &entry).await?;
    tracing::info!(path = relative_path, "downloaded");
    Ok(())
}
