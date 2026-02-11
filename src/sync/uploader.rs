use std::path::Path;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use crate::box_api::BoxClient;
use crate::util::hash;

use super::state::{self, SyncEntry};

/// Upload a new file to Box and record the synced state in the DB.
pub async fn upload_and_record(
    client: &BoxClient,
    pool: &SqlitePool,
    relative_path: &str,
    base_path: &Path,
    parent_id: &str,
    root_index: i64,
) -> Result<()> {
    let local_path = base_path.join(relative_path);
    let file_name = local_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Invalid file name")?;

    let local_sha1 = hash::compute_sha1(&local_path).await?;
    let local_meta = tokio::fs::metadata(&local_path).await?;
    let local_mtime = file_mtime_rfc3339(&local_meta)?;

    let remote_file = client
        .upload_file(&local_path, parent_id, file_name, Some(&local_mtime))
        .await
        .with_context(|| format!("Failed to upload {relative_path}"))?;

    let entry = SyncEntry {
        id: 0,
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
        last_sync_at: None,
        last_error: None,
        retry_count: 0,
    };

    state::upsert(pool, &entry).await?;
    tracing::info!(path = relative_path, "uploaded");
    Ok(())
}

/// Upload a new version of an existing file to Box and update the DB.
pub async fn upload_version_and_record(
    client: &BoxClient,
    pool: &SqlitePool,
    relative_path: &str,
    base_path: &Path,
    file_id: &str,
    etag: Option<&str>,
    root_index: i64,
) -> Result<()> {
    let local_path = base_path.join(relative_path);

    let local_sha1 = hash::compute_sha1(&local_path).await?;
    let local_meta = tokio::fs::metadata(&local_path).await?;
    let local_mtime = file_mtime_rfc3339(&local_meta)?;

    let remote_file = client
        .upload_new_version(file_id, etag, &local_path, Some(&local_mtime))
        .await
        .with_context(|| format!("Failed to upload new version of {relative_path}"))?;

    let entry = SyncEntry {
        id: 0,
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
        last_sync_at: None,
        last_error: None,
        retry_count: 0,
    };

    state::upsert(pool, &entry).await?;
    tracing::info!(path = relative_path, "uploaded new version");
    Ok(())
}

/// Convert a file's mtime to RFC 3339 string (second precision, UTC).
///
/// Box's API rejects fractional seconds with more than 3 digits,
/// so we truncate to whole seconds for maximum compatibility.
fn file_mtime_rfc3339(meta: &std::fs::Metadata) -> Result<String> {
    use chrono::SecondsFormat;
    use std::time::UNIX_EPOCH;
    let dur = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dt = chrono::DateTime::from_timestamp(dur.as_secs() as i64, 0).unwrap_or_default();
    Ok(dt.to_rfc3339_opts(SecondsFormat::Secs, true))
}
