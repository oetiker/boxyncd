pub mod conflict;
mod downloader;
pub mod local_watcher;
pub mod reconciler;
pub mod remote_watcher;
pub mod state;
mod uploader;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use crate::box_api::BoxClient;
use crate::config::Config;
use crate::util::{hash, path as sync_path};

use reconciler::{LocalEntry, RemoteEntry, SyncAction};
use state::SyncEntry;

/// The main sync engine that orchestrates full sync cycles.
pub struct SyncEngine {
    pool: SqlitePool,
    client: Arc<BoxClient>,
    config: Config,
    semaphore: Arc<Semaphore>,
}

impl SyncEngine {
    pub fn new(pool: SqlitePool, client: Arc<BoxClient>, config: Config) -> Self {
        let max_concurrent = config.general.max_concurrent_transfers;
        Self {
            pool,
            client,
            config,
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    /// Run a full sync cycle across all configured sync roots.
    pub async fn run_full_sync(&self) -> Result<()> {
        for (index, root) in self.config.sync.iter().enumerate() {
            let root_index = index as i64;
            tracing::info!(
                root = %root.local_path.display(),
                box_folder = %root.box_folder_id,
                "syncing root {}/{}", index + 1, self.config.sync.len(),
            );

            if let Err(e) = self.sync_root(root_index).await {
                tracing::error!(
                    root = %root.local_path.display(),
                    error = %e,
                    "sync root failed"
                );
            }
        }
        Ok(())
    }

    /// Sync a single root by index (public entry point for incremental sync).
    pub async fn sync_one_root(&self, root_index: i64) -> Result<()> {
        self.sync_root(root_index).await
    }

    /// Sync a single root: walk remote, walk local, reconcile, execute.
    async fn sync_root(&self, root_index: i64) -> Result<()> {
        let root = &self.config.sync[root_index as usize];
        let base_path = &root.local_path;

        // Ensure local root directory exists
        tokio::fs::create_dir_all(base_path)
            .await
            .with_context(|| format!("Failed to create sync root: {}", base_path.display()))?;

        // 1. Walk the remote tree
        tracing::debug!("walking remote tree");
        let remote_tree = self
            .walk_remote_tree(&root.box_folder_id, "", &root.exclude)
            .await?;
        tracing::debug!(count = remote_tree.len(), "remote entries found");

        // 2. Walk the local tree
        tracing::debug!("walking local tree");
        let local_tree = self.walk_local_tree(base_path, &root.exclude).await?;
        tracing::debug!(count = local_tree.len(), "local entries found");

        // 3. Load DB baseline
        let db_entries = state::get_all_for_root(&self.pool, root_index).await?;
        tracing::debug!(count = db_entries.len(), "DB entries loaded");

        // 4. Reconcile
        let actions = reconciler::reconcile(&local_tree, &remote_tree, &db_entries);
        if actions.is_empty() {
            tracing::info!("everything in sync");
            return Ok(());
        }
        tracing::info!(count = actions.len(), "actions to execute");

        // 5. Execute actions
        self.execute_actions(root_index, &actions, base_path, &remote_tree, &local_tree)
            .await?;

        Ok(())
    }

    /// Recursively walk the remote Box folder tree, building a flat list of RemoteEntry.
    fn walk_remote_tree<'a>(
        &'a self,
        folder_id: &'a str,
        prefix: &'a str,
        exclude: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<RemoteEntry>>> + Send + 'a>>
    {
        Box::pin(async move {
            let items = self.client.list_folder_items(folder_id).await?;
            let mut entries = Vec::new();

            for item in &items {
                let relative_path = if prefix.is_empty() {
                    item.name.clone()
                } else {
                    format!("{prefix}/{}", item.name)
                };

                // Check exclude patterns
                if sync_path::matches_exclude(&relative_path, exclude) {
                    tracing::debug!(path = %relative_path, "excluded (remote)");
                    continue;
                }

                if item.is_folder() {
                    entries.push(RemoteEntry {
                        relative_path: relative_path.clone(),
                        is_dir: true,
                        box_id: item.id.clone(),
                        parent_id: folder_id.to_string(),
                        sha1: None,
                        etag: item.etag.clone(),
                        size: 0,
                        modified_at: item.modified_at.clone(),
                    });

                    // Recurse into subfolder
                    let sub_entries = self
                        .walk_remote_tree(&item.id, &relative_path, exclude)
                        .await?;
                    entries.extend(sub_entries);
                } else if item.is_file() {
                    entries.push(RemoteEntry {
                        relative_path,
                        is_dir: false,
                        box_id: item.id.clone(),
                        parent_id: folder_id.to_string(),
                        sha1: item.sha1.clone(),
                        etag: item.etag.clone(),
                        size: item.size.unwrap_or(0),
                        modified_at: item
                            .content_modified_at
                            .clone()
                            .or(item.modified_at.clone()),
                    });
                }
                // Skip web_links and other types
            }

            Ok(entries)
        })
    }

    /// Recursively walk the local directory tree, building a flat list of LocalEntry.
    async fn walk_local_tree(
        &self,
        base_path: &Path,
        exclude: &[String],
    ) -> Result<Vec<LocalEntry>> {
        let mut entries = Vec::new();
        self.walk_local_dir(base_path, base_path, exclude, &mut entries)
            .await?;
        Ok(entries)
    }

    fn walk_local_dir<'a>(
        &'a self,
        base_path: &'a Path,
        dir: &'a Path,
        exclude: &'a [String],
        entries: &'a mut Vec<LocalEntry>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut read_dir = tokio::fs::read_dir(dir)
                .await
                .with_context(|| format!("Failed to read dir: {}", dir.display()))?;

            while let Some(entry) = read_dir.next_entry().await? {
                let path = entry.path();
                let meta = match tokio::fs::symlink_metadata(&path).await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!(path = %path.display(), error = %e, "cannot stat, skipping");
                        continue;
                    }
                };

                // Skip symlinks
                if meta.is_symlink() {
                    tracing::debug!(path = %path.display(), "skipping symlink");
                    continue;
                }

                let relative = sync_path::relative_path(base_path, &path)?;

                // Check exclude patterns
                if sync_path::matches_exclude(&relative, exclude) {
                    tracing::debug!(path = %relative, "excluded (local)");
                    continue;
                }

                if meta.is_dir() {
                    entries.push(LocalEntry {
                        relative_path: relative.clone(),
                        is_dir: true,
                        sha1: None,
                        size: 0,
                        modified_at: String::new(),
                    });

                    // Recurse
                    self.walk_local_dir(base_path, &path, exclude, entries)
                        .await?;
                } else if meta.is_file() {
                    let size = meta.len();
                    let mtime = {
                        use std::time::UNIX_EPOCH;
                        let dur = meta
                            .modified()
                            .unwrap_or(UNIX_EPOCH)
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default();
                        chrono::DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
                            .unwrap_or_default()
                            .to_rfc3339()
                    };

                    // Compute SHA-1 for content comparison
                    let sha1 = match hash::compute_sha1(&path).await {
                        Ok(h) => Some(h),
                        Err(e) => {
                            tracing::warn!(path = %relative, error = %e, "SHA-1 failed, skipping");
                            continue;
                        }
                    };

                    entries.push(LocalEntry {
                        relative_path: relative,
                        is_dir: false,
                        sha1,
                        size,
                        modified_at: mtime,
                    });
                }
            }

            Ok(())
        })
    }

    /// Execute a list of sync actions.
    async fn execute_actions(
        &self,
        root_index: i64,
        actions: &[SyncAction],
        base_path: &Path,
        remote_tree: &[RemoteEntry],
        local_tree: &[LocalEntry],
    ) -> Result<()> {
        // Build a lookup for remote entries (to resolve parent IDs)
        let remote_by_path: std::collections::HashMap<&str, &RemoteEntry> = remote_tree
            .iter()
            .map(|e| (e.relative_path.as_str(), e))
            .collect();

        for action in actions {
            let _permit = self.semaphore.acquire().await?;

            if let Err(e) = self
                .execute_action(action, root_index, base_path, &remote_by_path, local_tree)
                .await
            {
                tracing::error!(error = %e, action = ?action, "action failed");
                // Record the error for the relevant entry, but continue with other actions
                if let Some(path) = action_path(action)
                    && let Ok(Some(entry)) = state::get_by_path(&self.pool, root_index, path).await
                {
                    let _ = state::set_error(&self.pool, entry.id, &format!("{e:#}")).await;
                }
            }
        }

        Ok(())
    }

    /// Execute a single sync action.
    async fn execute_action(
        &self,
        action: &SyncAction,
        root_index: i64,
        base_path: &Path,
        remote_by_path: &std::collections::HashMap<&str, &RemoteEntry>,
        local_tree: &[LocalEntry],
    ) -> Result<()> {
        match action {
            SyncAction::Download {
                relative_path,
                box_id,
            } => {
                tracing::info!(path = %relative_path, "downloading");
                downloader::download_and_record(
                    &self.client,
                    &self.pool,
                    box_id,
                    relative_path,
                    base_path,
                    root_index,
                )
                .await?;
            }

            SyncAction::Upload {
                relative_path,
                parent_id,
            } => {
                // Resolve parent ID: if empty, look it up from the remote tree
                let resolved_parent = if parent_id.is_empty() {
                    self.resolve_parent_id(relative_path, root_index, remote_by_path)
                        .await?
                } else {
                    parent_id.clone()
                };

                tracing::info!(path = %relative_path, parent = %resolved_parent, "uploading");
                uploader::upload_and_record(
                    &self.client,
                    &self.pool,
                    relative_path,
                    base_path,
                    &resolved_parent,
                    root_index,
                )
                .await?;
            }

            SyncAction::UploadVersion {
                relative_path,
                file_id,
                etag,
            } => {
                tracing::info!(path = %relative_path, "uploading new version");
                uploader::upload_version_and_record(
                    &self.client,
                    &self.pool,
                    relative_path,
                    base_path,
                    file_id,
                    etag.as_deref(),
                    root_index,
                )
                .await?;
            }

            SyncAction::DeleteLocal { relative_path } => {
                let full_path = base_path.join(relative_path);
                tracing::info!(path = %relative_path, "deleting locally");

                let meta = tokio::fs::metadata(&full_path).await;
                match meta {
                    Ok(m) if m.is_dir() => {
                        tokio::fs::remove_dir_all(&full_path)
                            .await
                            .with_context(|| {
                                format!("Failed to remove dir: {}", full_path.display())
                            })?;
                    }
                    Ok(_) => {
                        tokio::fs::remove_file(&full_path).await.with_context(|| {
                            format!("Failed to remove file: {}", full_path.display())
                        })?;
                    }
                    Err(_) => {
                        tracing::debug!(path = %relative_path, "already gone locally");
                    }
                }

                // Remove from DB
                if let Some(entry) =
                    state::get_by_path(&self.pool, root_index, relative_path).await?
                {
                    state::delete_entry(&self.pool, entry.id).await?;
                }
            }

            SyncAction::DeleteRemote {
                box_id,
                etag,
                is_dir,
            } => {
                tracing::info!(box_id = %box_id, "deleting on Box");
                if *is_dir {
                    self.client
                        .delete_folder(box_id, etag.as_deref(), true)
                        .await?;
                } else {
                    self.client.delete_file(box_id, etag.as_deref()).await?;
                }

                // Find and remove from DB by box_id
                // We need to search by box_id; use a query
                let row = sqlx::query(
                    "SELECT id FROM sync_entries WHERE box_id = ? AND sync_root_index = ?",
                )
                .bind(box_id)
                .bind(root_index)
                .fetch_optional(&self.pool)
                .await?;
                if let Some(row) = row {
                    use sqlx::Row;
                    let entry_id: i64 = row.get("id");
                    state::delete_entry(&self.pool, entry_id).await?;
                }
            }

            SyncAction::CreateLocalDir { relative_path } => {
                let full_path = base_path.join(relative_path);
                tracing::info!(path = %relative_path, "creating local directory");
                tokio::fs::create_dir_all(&full_path)
                    .await
                    .with_context(|| format!("Failed to create dir: {}", full_path.display()))?;

                // Record the directory in DB
                if let Some(remote) = remote_by_path.get(relative_path.as_str()) {
                    let entry = SyncEntry {
                        id: 0,
                        sync_root_index: root_index,
                        box_id: Some(remote.box_id.clone()),
                        box_parent_id: Some(remote.parent_id.clone()),
                        entry_type: "folder".into(),
                        relative_path: relative_path.clone(),
                        local_sha1: None,
                        remote_sha1: None,
                        remote_etag: remote.etag.clone(),
                        remote_version_id: None,
                        remote_modified_at: remote.modified_at.clone(),
                        local_modified_at: None,
                        local_size: None,
                        sync_status: "synced".into(),
                        last_sync_at: None,
                        last_error: None,
                        retry_count: 0,
                    };
                    state::upsert(&self.pool, &entry).await?;
                }
            }

            SyncAction::CreateRemoteDir {
                relative_path,
                parent_id,
            } => {
                let dir_name = relative_path.rsplit('/').next().unwrap_or(relative_path);

                let resolved_parent = if parent_id.is_empty() {
                    self.resolve_parent_id(relative_path, root_index, remote_by_path)
                        .await?
                } else {
                    parent_id.clone()
                };

                tracing::info!(path = %relative_path, "creating remote directory");
                let folder = self
                    .client
                    .create_folder(dir_name, &resolved_parent)
                    .await?;

                let entry = SyncEntry {
                    id: 0,
                    sync_root_index: root_index,
                    box_id: Some(folder.id),
                    box_parent_id: Some(resolved_parent),
                    entry_type: "folder".into(),
                    relative_path: relative_path.clone(),
                    local_sha1: None,
                    remote_sha1: None,
                    remote_etag: folder.etag,
                    remote_version_id: None,
                    remote_modified_at: None,
                    local_modified_at: None,
                    local_size: None,
                    sync_status: "synced".into(),
                    last_sync_at: None,
                    last_error: None,
                    retry_count: 0,
                };
                state::upsert(&self.pool, &entry).await?;
            }

            SyncAction::Conflict {
                relative_path,
                box_id,
            } => {
                // Download the remote version under a conflict name,
                // keep the local version as-is.
                let conflict_rel = conflict::conflict_path(relative_path);
                tracing::warn!(
                    path = %relative_path,
                    conflict_copy = %conflict_rel,
                    "conflict detected — downloading remote as conflict copy"
                );

                downloader::download_and_record(
                    &self.client,
                    &self.pool,
                    box_id,
                    &conflict_rel,
                    base_path,
                    root_index,
                )
                .await?;

                // Also record the local version's current state
                let local_path = base_path.join(relative_path);
                if local_path.exists() {
                    let local_sha1 = hash::compute_sha1(&local_path).await?;
                    let local_meta = tokio::fs::metadata(&local_path).await?;
                    let local_mtime = {
                        use std::time::UNIX_EPOCH;
                        let dur = local_meta
                            .modified()
                            .unwrap_or(UNIX_EPOCH)
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default();
                        chrono::DateTime::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
                            .unwrap_or_default()
                            .to_rfc3339()
                    };

                    // Find the remote entry for this path
                    let remote = remote_by_path.get(relative_path.as_str());
                    let entry = SyncEntry {
                        id: 0,
                        sync_root_index: root_index,
                        box_id: remote.map(|r| r.box_id.clone()),
                        box_parent_id: remote.map(|r| r.parent_id.clone()),
                        entry_type: "file".into(),
                        relative_path: relative_path.clone(),
                        local_sha1: Some(local_sha1),
                        remote_sha1: remote.and_then(|r| r.sha1.clone()),
                        remote_etag: remote.and_then(|r| r.etag.clone()),
                        remote_version_id: None,
                        remote_modified_at: remote.and_then(|r| r.modified_at.clone()),
                        local_modified_at: Some(local_mtime),
                        local_size: Some(local_meta.len() as i64),
                        sync_status: "conflict".into(),
                        last_sync_at: None,
                        last_error: None,
                        retry_count: 0,
                    };
                    state::upsert(&self.pool, &entry).await?;
                }
            }

            SyncAction::RemoveDbEntry { entry_id } => {
                tracing::debug!(entry_id, "removing stale DB entry");
                state::delete_entry(&self.pool, *entry_id).await?;
            }

            SyncAction::RecordSynced { relative_path } => {
                tracing::debug!(path = %relative_path, "recording as synced");
                // Build entry from current local + remote state
                let remote = remote_by_path.get(relative_path.as_str());
                let local = local_tree
                    .iter()
                    .find(|e| e.relative_path == *relative_path);

                let entry = SyncEntry {
                    id: 0,
                    sync_root_index: root_index,
                    box_id: remote.map(|r| r.box_id.clone()),
                    box_parent_id: remote.map(|r| r.parent_id.clone()),
                    entry_type: if local.is_some_and(|l| l.is_dir) {
                        "folder"
                    } else {
                        "file"
                    }
                    .into(),
                    relative_path: relative_path.clone(),
                    local_sha1: local.and_then(|l| l.sha1.clone()),
                    remote_sha1: remote.and_then(|r| r.sha1.clone()),
                    remote_etag: remote.and_then(|r| r.etag.clone()),
                    remote_version_id: None,
                    remote_modified_at: remote.and_then(|r| r.modified_at.clone()),
                    local_modified_at: local.map(|l| l.modified_at.clone()),
                    local_size: local.map(|l| l.size as i64),
                    sync_status: "synced".into(),
                    last_sync_at: None,
                    last_error: None,
                    retry_count: 0,
                };
                state::upsert(&self.pool, &entry).await?;
            }
        }

        Ok(())
    }

    /// Resolve the Box parent folder ID for a given relative path.
    ///
    /// Looks up the parent directory in the remote tree or DB.
    /// For root-level files, returns the sync root's box_folder_id.
    async fn resolve_parent_id(
        &self,
        relative_path: &str,
        root_index: i64,
        remote_by_path: &std::collections::HashMap<&str, &RemoteEntry>,
    ) -> Result<String> {
        // If the file is at root level (no '/'), use the sync root folder ID
        let parent_rel = match relative_path.rsplit_once('/') {
            Some((parent, _)) => parent,
            None => {
                return Ok(self.config.sync[root_index as usize].box_folder_id.clone());
            }
        };

        // Check remote tree first
        if let Some(remote) = remote_by_path.get(parent_rel) {
            return Ok(remote.box_id.clone());
        }

        // Check DB
        if let Some(entry) = state::get_by_path(&self.pool, root_index, parent_rel).await?
            && let Some(box_id) = entry.box_id
        {
            return Ok(box_id);
        }

        anyhow::bail!(
            "Cannot resolve parent Box folder ID for '{relative_path}'. \
             Parent directory '{parent_rel}' not found in remote tree or DB."
        );
    }
}

/// Extract the relative path from a SyncAction, if applicable.
fn action_path(action: &SyncAction) -> Option<&str> {
    match action {
        SyncAction::Upload { relative_path, .. }
        | SyncAction::UploadVersion { relative_path, .. }
        | SyncAction::Download { relative_path, .. }
        | SyncAction::DeleteLocal { relative_path }
        | SyncAction::CreateLocalDir { relative_path }
        | SyncAction::CreateRemoteDir { relative_path, .. }
        | SyncAction::Conflict { relative_path, .. }
        | SyncAction::RecordSynced { relative_path } => Some(relative_path),
        SyncAction::DeleteRemote { .. } | SyncAction::RemoveDbEntry { .. } => None,
    }
}
