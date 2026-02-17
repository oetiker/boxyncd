use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sqlx::SqlitePool;

use super::state;

/// Move a file or directory to the local trash bin.
///
/// Files are placed under date-stamped folders: `<trash_base>/YYYY-MM-DD/<relative_path>`.
/// If a name clash occurs, falls back to `<trash_base>/YYYY-MM-DD-HH-MM-SS/<relative_path>`.
///
/// Returns the full path where the item was moved to.
pub async fn move_to_trash(
    trash_base: &Path,
    relative_path: &str,
    full_path: &Path,
) -> Result<PathBuf> {
    let now = chrono::Utc::now();

    // Try daily folder first
    let date_folder = now.format("%Y-%m-%d").to_string();
    let dest = trash_base.join(&date_folder).join(relative_path);

    let dest = if dest.exists() {
        // Name clash — fall back to precise timestamp
        let precise_folder = now.format("%Y-%m-%d-%H-%M-%S").to_string();
        trash_base.join(&precise_folder).join(relative_path)
    } else {
        dest
    };

    // Create parent directories
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create trash directory: {}", parent.display()))?;
    }

    // Move the file/directory
    move_entry(full_path, &dest).await.with_context(|| {
        format!(
            "Failed to move {} to trash at {}",
            full_path.display(),
            dest.display()
        )
    })?;

    tracing::info!(
        from = %full_path.display(),
        to = %dest.display(),
        "moved to trash"
    );

    Ok(dest)
}

/// Move a file or directory, falling back to copy+delete for cross-filesystem moves.
async fn move_entry(src: &Path, dst: &Path) -> Result<()> {
    match tokio::fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(e) if e.raw_os_error() == Some(18 /* EXDEV */) => {
            // Cross-filesystem: copy then delete
            copy_recursive(src, dst).await?;
            if src.is_dir() {
                tokio::fs::remove_dir_all(src).await?;
            } else {
                tokio::fs::remove_file(src).await?;
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Recursively copy a file or directory tree.
fn copy_recursive<'a>(
    src: &'a Path,
    dst: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let meta = tokio::fs::symlink_metadata(src).await?;

        if meta.is_dir() {
            tokio::fs::create_dir_all(dst).await?;
            let mut read_dir = tokio::fs::read_dir(src).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                let src_child = entry.path();
                let dst_child = dst.join(entry.file_name());
                copy_recursive(&src_child, &dst_child).await?;
            }
        } else {
            tokio::fs::copy(src, dst).await?;
        }

        Ok(())
    })
}

/// Remove expired trash entries: delete files from disk and entries from DB.
pub async fn cleanup_trash(
    pool: &SqlitePool,
    root_index: i64,
    trash_base: &Path,
    max_days: u64,
) -> Result<()> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(max_days as i64);
    let entries = state::get_expired_trash(pool, root_index, &cutoff.to_rfc3339()).await?;

    if entries.is_empty() {
        return Ok(());
    }

    tracing::info!(
        count = entries.len(),
        days = max_days,
        "cleaning up expired trash entries"
    );

    for entry in &entries {
        if let Some(ref tp) = entry.trash_path {
            let path = PathBuf::from(tp);
            if path.exists() {
                let result = if path.is_dir() {
                    tokio::fs::remove_dir_all(&path).await
                } else {
                    tokio::fs::remove_file(&path).await
                };
                match result {
                    Ok(()) => tracing::debug!(path = %path.display(), "removed expired trash"),
                    Err(e) => tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "failed to remove expired trash file"
                    ),
                }
            }
        }
        state::delete_entry(pool, entry.id).await?;
    }

    // Clean up empty date folders left behind
    cleanup_empty_dirs(trash_base).await;

    Ok(())
}

/// Recursively remove empty subdirectories in the trash base (bottom-up).
fn cleanup_empty_dirs(
    dir: &Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        let Ok(mut read_dir) = tokio::fs::read_dir(dir).await else {
            return;
        };

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                // Recurse first (bottom-up) so children are cleaned before parents
                cleanup_empty_dirs(&path).await;
                // remove_dir only succeeds on empty directories
                let _ = tokio::fs::remove_dir(&path).await;
            }
        }
    })
}
