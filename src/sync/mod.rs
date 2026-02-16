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
pub use remote_watcher::RemoteEvent;
use state::SyncEntry;

/// Result of a sync cycle — tells the event loop whether we changed anything
/// on Box so it can suppress the resulting remote watcher echo.
#[derive(Debug, Default)]
pub struct SyncOutcome {
    /// Whether we modified anything on Box (upload, delete, create folder).
    pub modified_remote: bool,
}

impl SyncOutcome {
    fn merge(&mut self, other: SyncOutcome) {
        self.modified_remote |= other.modified_remote;
    }
}

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

    /// Dry-run: walk trees, reconcile, and return actions per root without executing.
    pub async fn dry_run(&self) -> Vec<(usize, Result<Vec<SyncAction>>)> {
        let mut results = Vec::new();
        for (index, root) in self.config.sync.iter().enumerate() {
            let root_index = index as i64;
            let base_path = &root.local_path;

            let result = async {
                // 1. Walk the remote tree
                let remote_tree = self
                    .walk_remote_tree(&root.box_folder_id, "", &root.exclude)
                    .await?;

                // 2. Load DB baseline
                let db_entries = state::get_all_for_root(&self.pool, root_index).await?;
                let db_by_path: std::collections::HashMap<&str, &SyncEntry> = db_entries
                    .iter()
                    .map(|e| (e.relative_path.as_str(), e))
                    .collect();

                // 3. Walk the local tree
                let local_tree = self
                    .walk_local_tree(base_path, &root.exclude, &db_by_path)
                    .await?;

                // 4. Reconcile (no execution)
                let actions = reconciler::reconcile(&local_tree, &remote_tree, &db_entries);
                Ok(actions)
            }
            .await;

            results.push((index, result));
        }
        results
    }

    /// Run a full sync cycle across all configured sync roots.
    pub async fn run_full_sync(&self) -> Result<SyncOutcome> {
        let mut outcome = SyncOutcome::default();
        for (index, root) in self.config.sync.iter().enumerate() {
            let root_index = index as i64;
            tracing::info!(
                root = %root.local_path.display(),
                box_folder = %root.box_folder_id,
                "syncing root {}/{}", index + 1, self.config.sync.len(),
            );

            match self.sync_root(root_index).await {
                Ok(root_outcome) => outcome.merge(root_outcome),
                Err(e) => {
                    tracing::error!(
                        root = %root.local_path.display(),
                        error = %e,
                        "sync root failed"
                    );
                }
            }
        }
        Ok(outcome)
    }

    /// Sync a single root: walk remote, walk local, reconcile, execute.
    async fn sync_root(&self, root_index: i64) -> Result<SyncOutcome> {
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

        // 2. Load DB baseline (needed before local walk for ctime optimisation)
        let db_entries = state::get_all_for_root(&self.pool, root_index).await?;
        tracing::debug!(count = db_entries.len(), "DB entries loaded");
        let db_by_path: std::collections::HashMap<&str, &SyncEntry> = db_entries
            .iter()
            .map(|e| (e.relative_path.as_str(), e))
            .collect();

        // 3. Walk the local tree (skips SHA1 for files unchanged since last sync)
        tracing::debug!("walking local tree");
        let local_tree = self
            .walk_local_tree(base_path, &root.exclude, &db_by_path)
            .await?;
        tracing::debug!(count = local_tree.len(), "local entries found");

        // 4. Reconcile
        let actions = reconciler::reconcile(&local_tree, &remote_tree, &db_entries);
        if actions.is_empty() {
            tracing::info!("everything in sync");
            return Ok(SyncOutcome::default());
        }
        tracing::info!(count = actions.len(), "actions to execute");

        // 5. Execute actions
        self.execute_actions(root_index, &actions, base_path, &remote_tree, &local_tree)
            .await
    }

    /// Walk the remote Box folder tree concurrently, building a flat list of RemoteEntry.
    ///
    /// Uses a bounded `JoinSet` (up to 4 concurrent API calls) so that large
    /// hierarchies are listed in parallel instead of sequentially.
    async fn walk_remote_tree(
        &self,
        folder_id: &str,
        prefix: &str,
        exclude: &[String],
    ) -> Result<Vec<RemoteEntry>> {
        let api_sem = Arc::new(Semaphore::new(4));
        let client = Arc::clone(&self.client);
        let exclude: Arc<[String]> = exclude.to_vec().into();
        let mut entries = Vec::new();
        let mut join_set = tokio::task::JoinSet::new();

        // Seed with the root folder
        {
            let sem = Arc::clone(&api_sem);
            let cl = Arc::clone(&client);
            let exc = Arc::clone(&exclude);
            let fid = folder_id.to_string();
            let pfx = prefix.to_string();
            join_set.spawn(async move {
                let _permit = sem.acquire().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                list_remote_folder(&cl, &fid, &pfx, &exc).await
            });
        }

        while let Some(result) = join_set.join_next().await {
            let (folder_entries, subfolders) =
                result.context("remote tree walk task panicked")??;
            entries.extend(folder_entries);

            for (sub_id, sub_prefix) in subfolders {
                let sem = Arc::clone(&api_sem);
                let cl = Arc::clone(&client);
                let exc = Arc::clone(&exclude);
                join_set.spawn(async move {
                    let _permit = sem.acquire().await.map_err(|e| anyhow::anyhow!("{e}"))?;
                    list_remote_folder(&cl, &sub_id, &sub_prefix, &exc).await
                });
            }
        }

        Ok(entries)
    }

    /// Recursively walk the local directory tree, building a flat list of LocalEntry.
    /// Uses DB entries to skip SHA1 computation for files unchanged since last sync
    /// (based on inode ctime), avoiding unnecessary file reads and inotify noise.
    async fn walk_local_tree<'a>(
        &self,
        base_path: &Path,
        exclude: &[String],
        db_by_path: &std::collections::HashMap<&'a str, &'a SyncEntry>,
    ) -> Result<Vec<LocalEntry>> {
        let mut entries = Vec::new();
        self.walk_local_dir(base_path, base_path, exclude, &mut entries, db_by_path)
            .await?;
        Ok(entries)
    }

    fn walk_local_dir<'a>(
        &'a self,
        base_path: &'a Path,
        dir: &'a Path,
        exclude: &'a [String],
        entries: &'a mut Vec<LocalEntry>,
        db_by_path: &'a std::collections::HashMap<&'a str, &'a SyncEntry>,
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
                    self.walk_local_dir(base_path, &path, exclude, entries, db_by_path)
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

                    // If the file's inode hasn't changed since the last sync
                    // (ctime <= last_sync_at), reuse the cached SHA1 from the DB
                    // instead of reading the file — avoids I/O and inotify noise.
                    let sha1 = if let Some(cached) = cached_sha1(&meta, &relative, db_by_path) {
                        cached
                    } else {
                        match hash::compute_sha1(&path).await {
                            Ok(h) => Some(h),
                            Err(e) => {
                                tracing::warn!(
                                    path = %relative, error = %e, "SHA-1 failed, skipping"
                                );
                                continue;
                            }
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

    /// Execute a list of sync actions, returning what was touched for echo suppression.
    async fn execute_actions(
        &self,
        root_index: i64,
        actions: &[SyncAction],
        base_path: &Path,
        remote_tree: &[RemoteEntry],
        local_tree: &[LocalEntry],
    ) -> Result<SyncOutcome> {
        // Build a lookup for remote entries (to resolve parent IDs)
        let remote_by_path: std::collections::HashMap<&str, &RemoteEntry> = remote_tree
            .iter()
            .map(|e| (e.relative_path.as_str(), e))
            .collect();

        let mut outcome = SyncOutcome::default();

        for action in actions {
            let _permit = self.semaphore.acquire().await?;

            match self
                .execute_action(action, root_index, base_path, &remote_by_path, local_tree)
                .await
            {
                Ok(()) => {
                    if matches!(
                        action,
                        SyncAction::Upload { .. }
                            | SyncAction::UploadVersion { .. }
                            | SyncAction::DeleteRemote { .. }
                            | SyncAction::CreateRemoteDir { .. }
                    ) {
                        outcome.modified_remote = true;
                    }
                }
                Err(e) => {
                    tracing::error!(error = format!("{e:#}"), action = ?action, "action failed");
                    if let Some(path) = action_path(action)
                        && let Ok(Some(entry)) =
                            state::get_by_path(&self.pool, root_index, path).await
                    {
                        let _ = state::set_error(&self.pool, entry.id, &format!("{e:#}")).await;
                    }
                }
            }
        }

        Ok(outcome)
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

    // ── Targeted (per-file) sync ────────────────────────────────────

    /// Sync a single locally-changed file without walking the tree.
    pub async fn sync_local_change(
        &self,
        root_index: i64,
        relative_path: &str,
    ) -> Result<SyncOutcome> {
        let root = &self.config.sync[root_index as usize];
        let base_path = &root.local_path;

        tracing::info!(path = %relative_path, "targeted local sync");

        // 1. DB baseline
        let db_entry = state::get_by_path(&self.pool, root_index, relative_path).await?;

        // 2. Local state
        let local = self.stat_local_entry(base_path, relative_path).await;

        // 3. Remote state
        let remote = if let Some(ref db) = db_entry
            && let Some(ref box_id) = db.box_id
        {
            self.fetch_remote_entry(box_id, relative_path).await?
        } else {
            // New local file — check if it already exists on Box
            self.find_remote_entry(root_index, relative_path).await?
        };

        // 4. Reconcile
        let action = reconciler::reconcile_entry(
            relative_path,
            local.as_ref(),
            remote.as_ref(),
            db_entry.as_ref(),
        );

        let Some(action) = action else {
            tracing::debug!(path = %relative_path, "already in sync");
            return Ok(SyncOutcome::default());
        };

        tracing::debug!(path = %relative_path, action = ?action, "targeted action");

        // 5. Execute
        self.execute_single_action(
            root_index,
            &action,
            base_path,
            remote.as_ref(),
            local.as_ref(),
        )
        .await
    }

    /// Sync a single remotely-changed file without walking the tree.
    pub async fn sync_remote_change(
        &self,
        root_index: i64,
        event: &RemoteEvent,
    ) -> Result<SyncOutcome> {
        let root = &self.config.sync[root_index as usize];
        let base_path = &root.local_path;

        // 1. Determine relative_path
        let relative_path = if let Some(ref box_id) = event.box_id
            && let Some(db) = state::get_by_box_id(&self.pool, root_index, box_id).await?
        {
            db.relative_path.clone()
        } else {
            // Item not in DB — resolve from event metadata
            match self.resolve_relative_path(root_index, event).await {
                Some(p) => p,
                None => {
                    tracing::debug!(
                        event_type = %event.event_type,
                        box_id = ?event.box_id,
                        "cannot resolve path for remote event, skipping"
                    );
                    return Ok(SyncOutcome::default());
                }
            }
        };

        tracing::info!(path = %relative_path, event_type = %event.event_type, "targeted remote sync");

        // 2. Remote state
        let remote = if event.event_type == "ITEM_TRASH" {
            // We know it's gone — skip the API call
            None
        } else if let Some(ref box_id) = event.box_id {
            self.fetch_remote_entry(box_id, &relative_path).await?
        } else {
            None
        };

        // 3. Local state
        let local = self.stat_local_entry(base_path, &relative_path).await;

        // 4. DB baseline
        let db_entry = state::get_by_path(&self.pool, root_index, &relative_path).await?;

        // 5. Reconcile
        let action = reconciler::reconcile_entry(
            &relative_path,
            local.as_ref(),
            remote.as_ref(),
            db_entry.as_ref(),
        );

        let Some(action) = action else {
            tracing::debug!(path = %relative_path, "already in sync");
            return Ok(SyncOutcome::default());
        };

        tracing::debug!(path = %relative_path, action = ?action, "targeted action");

        // 6. Execute
        self.execute_single_action(
            root_index,
            &action,
            base_path,
            remote.as_ref(),
            local.as_ref(),
        )
        .await
    }

    /// Stat a single local file and build a `LocalEntry` (None if it doesn't exist).
    async fn stat_local_entry(&self, base_path: &Path, relative_path: &str) -> Option<LocalEntry> {
        let full_path = base_path.join(relative_path);
        let meta = tokio::fs::symlink_metadata(&full_path).await.ok()?;

        if meta.is_symlink() {
            return None;
        }

        if meta.is_dir() {
            return Some(LocalEntry {
                relative_path: relative_path.to_string(),
                is_dir: true,
                sha1: None,
                size: 0,
                modified_at: String::new(),
            });
        }

        if meta.is_file() {
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
            let sha1 = match hash::compute_sha1(&full_path).await {
                Ok(h) => Some(h),
                Err(e) => {
                    tracing::warn!(path = %relative_path, error = %e, "SHA-1 failed");
                    return None;
                }
            };
            return Some(LocalEntry {
                relative_path: relative_path.to_string(),
                is_dir: false,
                sha1,
                size,
                modified_at: mtime,
            });
        }

        None
    }

    /// Fetch remote file metadata from Box and wrap as RemoteEntry.
    /// Returns None on 404 / trashed items.
    async fn fetch_remote_entry(
        &self,
        box_id: &str,
        relative_path: &str,
    ) -> Result<Option<RemoteEntry>> {
        match self.client.get_file(box_id).await {
            Ok(f) => {
                if f.item_status.as_deref() == Some("trashed") {
                    return Ok(None);
                }
                Ok(Some(RemoteEntry {
                    relative_path: relative_path.to_string(),
                    is_dir: false,
                    box_id: f.id,
                    parent_id: f.parent.map(|p| p.id).unwrap_or_default(),
                    sha1: f.sha1,
                    etag: f.etag,
                    size: f.size,
                    modified_at: f.content_modified_at.or(f.modified_at),
                }))
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("404") || msg.contains("not_found") {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// For new local files not yet in DB: resolve parent folder's box_id,
    /// list its items, and search by name.
    async fn find_remote_entry(
        &self,
        root_index: i64,
        relative_path: &str,
    ) -> Result<Option<RemoteEntry>> {
        let file_name = relative_path.rsplit('/').next().unwrap_or(relative_path);
        let parent_box_id = self
            .resolve_parent_id_from_db(relative_path, root_index)
            .await?;

        let Some(parent_box_id) = parent_box_id else {
            return Ok(None);
        };

        let items = self.client.list_folder_items(&parent_box_id).await?;
        for item in &items {
            if item.name == file_name {
                return Ok(Some(RemoteEntry {
                    relative_path: relative_path.to_string(),
                    is_dir: item.is_folder(),
                    box_id: item.id.clone(),
                    parent_id: parent_box_id.clone(),
                    sha1: item.sha1.clone(),
                    etag: item.etag.clone(),
                    size: item.size.unwrap_or(0),
                    modified_at: item
                        .content_modified_at
                        .clone()
                        .or(item.modified_at.clone()),
                }));
            }
        }

        Ok(None)
    }

    /// Resolve parent box_id from DB or config (for targeted sync, no remote_by_path map).
    async fn resolve_parent_id_from_db(
        &self,
        relative_path: &str,
        root_index: i64,
    ) -> Result<Option<String>> {
        let parent_rel = match relative_path.rsplit_once('/') {
            Some((parent, _)) => parent,
            None => {
                return Ok(Some(
                    self.config.sync[root_index as usize].box_folder_id.clone(),
                ));
            }
        };

        if let Some(entry) = state::get_by_path(&self.pool, root_index, parent_rel).await?
            && let Some(box_id) = entry.box_id
        {
            return Ok(Some(box_id));
        }

        Ok(None)
    }

    /// Resolve a relative_path for a remote event whose item isn't in DB.
    async fn resolve_relative_path(&self, root_index: i64, event: &RemoteEvent) -> Option<String> {
        let name = event.name.as_deref()?;
        let parent_id = event.parent_id.as_deref()?;

        // Check if parent_id is the root folder
        let root_folder_id = &self.config.sync[root_index as usize].box_folder_id;
        if parent_id == root_folder_id {
            return Some(name.to_string());
        }

        // Look up parent in DB to get its relative_path
        if let Ok(Some(parent_entry)) = state::get_by_box_id_any_root(&self.pool, parent_id).await {
            return Some(format!("{}/{name}", parent_entry.relative_path));
        }

        None
    }

    /// Execute a single action for targeted sync (no full remote/local trees).
    async fn execute_single_action(
        &self,
        root_index: i64,
        action: &SyncAction,
        base_path: &Path,
        remote: Option<&RemoteEntry>,
        local: Option<&LocalEntry>,
    ) -> Result<SyncOutcome> {
        let _permit = self.semaphore.acquire().await?;
        let mut outcome = SyncOutcome::default();

        // Build a minimal remote_by_path map for execute_action
        let remote_vec: Vec<RemoteEntry> = remote.cloned().into_iter().collect();
        let remote_by_path: std::collections::HashMap<&str, &RemoteEntry> = remote_vec
            .iter()
            .map(|e| (e.relative_path.as_str(), e))
            .collect();
        let local_vec: Vec<LocalEntry> = local.cloned().into_iter().collect();

        match self
            .execute_action(action, root_index, base_path, &remote_by_path, &local_vec)
            .await
        {
            Ok(()) => {
                if matches!(
                    action,
                    SyncAction::Upload { .. }
                        | SyncAction::UploadVersion { .. }
                        | SyncAction::DeleteRemote { .. }
                        | SyncAction::CreateRemoteDir { .. }
                ) {
                    outcome.modified_remote = true;
                }
            }
            Err(e) => {
                tracing::error!(error = format!("{e:#}"), action = ?action, "targeted action failed");
                if let Some(path) = action_path(action)
                    && let Ok(Some(entry)) = state::get_by_path(&self.pool, root_index, path).await
                {
                    let _ = state::set_error(&self.pool, entry.id, &format!("{e:#}")).await;
                }
                return Err(e);
            }
        }

        Ok(outcome)
    }
}

/// Check whether a file's inode is unchanged since the last sync by comparing
/// ctime (nanosecond precision) against the DB's `last_sync_at`.  If unchanged,
/// return the cached SHA1 from the DB — avoids reading the file and generating
/// inotify noise from the atime update.
fn cached_sha1(
    meta: &std::fs::Metadata,
    relative: &str,
    db_by_path: &std::collections::HashMap<&str, &SyncEntry>,
) -> Option<Option<String>> {
    use std::os::unix::fs::MetadataExt;

    let entry = db_by_path.get(relative)?;
    let sync_ns = entry
        .last_sync_at
        .as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp() as i128 * 1_000_000_000 + dt.timestamp_subsec_nanos() as i128)?;

    let file_ctime_ns = meta.ctime() as i128 * 1_000_000_000 + meta.ctime_nsec() as i128;

    if file_ctime_ns <= sync_ns {
        tracing::trace!(path = %relative, "ctime unchanged, reusing cached SHA1");
        Some(entry.local_sha1.clone())
    } else {
        None
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

/// List a single remote folder, returning its entries and subfolders to recurse into.
async fn list_remote_folder(
    client: &BoxClient,
    folder_id: &str,
    prefix: &str,
    exclude: &[String],
) -> Result<(Vec<RemoteEntry>, Vec<(String, String)>)> {
    let items = client.list_folder_items(folder_id).await?;

    let prefix_display = if prefix.is_empty() { "/" } else { prefix };
    tracing::debug!(folder = %prefix_display, items = items.len(), "listed remote folder");

    let mut entries = Vec::new();
    let mut subfolders = Vec::new();

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

            subfolders.push((item.id.clone(), relative_path));
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

    Ok((entries, subfolders))
}
