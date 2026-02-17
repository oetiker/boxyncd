use std::collections::{BTreeMap, HashSet};

use super::state::SyncEntry;

/// Snapshot of a locally-observed file or directory.
#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub relative_path: String,
    pub is_dir: bool,
    /// SHA-1 hex digest (None for directories).
    pub sha1: Option<String>,
    pub size: u64,
    /// ISO 8601 mtime string.
    pub modified_at: String,
}

/// Snapshot of a remotely-observed file or directory from Box.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RemoteEntry {
    pub relative_path: String,
    pub is_dir: bool,
    pub box_id: String,
    pub parent_id: String,
    pub sha1: Option<String>,
    pub etag: Option<String>,
    pub size: u64,
    pub modified_at: Option<String>,
}

/// An action the sync engine should execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Upload a new file to Box.
    Upload {
        relative_path: String,
        parent_id: String,
    },
    /// Upload a new version of an existing file.
    UploadVersion {
        relative_path: String,
        file_id: String,
        etag: Option<String>,
    },
    /// Download a file from Box to local disk.
    Download {
        relative_path: String,
        box_id: String,
    },
    /// Delete a local file/folder.
    DeleteLocal { relative_path: String },
    /// Delete a remote file/folder on Box.
    DeleteRemote {
        box_id: String,
        etag: Option<String>,
        is_dir: bool,
    },
    /// Create a directory locally.
    CreateLocalDir { relative_path: String },
    /// Create a directory on Box.
    CreateRemoteDir {
        relative_path: String,
        parent_id: String,
    },
    /// Both sides changed — create a conflicted copy.
    Conflict {
        relative_path: String,
        box_id: String,
    },
    /// Remove a DB entry that no longer exists on either side.
    RemoveDbEntry { entry_id: i64 },
    /// Record the current state to DB without any transfer (already in sync).
    RecordSynced { relative_path: String },
}

/// Run the three-way reconciliation algorithm.
///
/// Compares the current local tree, current remote tree, and the DB baseline
/// to produce a list of actions needed to bring both sides into sync.
pub fn reconcile(
    local_tree: &[LocalEntry],
    remote_tree: &[RemoteEntry],
    db_entries: &[SyncEntry],
) -> Vec<SyncAction> {
    let local_map: BTreeMap<&str, &LocalEntry> = local_tree
        .iter()
        .map(|e| (e.relative_path.as_str(), e))
        .collect();
    let remote_map: BTreeMap<&str, &RemoteEntry> = remote_tree
        .iter()
        .map(|e| (e.relative_path.as_str(), e))
        .collect();
    let db_map: BTreeMap<&str, &SyncEntry> = db_entries
        .iter()
        .map(|e| (e.relative_path.as_str(), e))
        .collect();

    // Collect all known paths
    let mut all_paths: HashSet<&str> = HashSet::new();
    all_paths.extend(local_map.keys());
    all_paths.extend(remote_map.keys());
    all_paths.extend(db_map.keys());

    // Sort paths for deterministic ordering: directories before files (breadth-first).
    let mut sorted_paths: Vec<&str> = all_paths.into_iter().collect();
    sorted_paths.sort_by(|a, b| {
        let a_depth = a.matches('/').count();
        let b_depth = b.matches('/').count();
        a_depth.cmp(&b_depth).then(a.cmp(b))
    });

    let mut actions = Vec::new();

    for path in sorted_paths {
        let local = local_map.get(path).copied();
        let remote = remote_map.get(path).copied();
        let db = db_map.get(path).copied();

        let action = reconcile_entry(path, local, remote, db);
        if let Some(a) = action {
            actions.push(a);
        }
    }

    actions
}

/// Reconcile a single path across local, remote, and DB.
pub fn reconcile_entry(
    path: &str,
    local: Option<&LocalEntry>,
    remote: Option<&RemoteEntry>,
    db: Option<&SyncEntry>,
) -> Option<SyncAction> {
    match (local, remote, db) {
        // --- Exists in all three: check for changes ---
        (Some(l), Some(r), Some(d)) => reconcile_three_way(path, l, r, d),

        // --- Not in DB (initial sync or new on both sides) ---
        (Some(l), Some(r), None) => {
            // Both sides have it but DB doesn't know about it yet.
            if l.is_dir && r.is_dir {
                // Both are directories — just record.
                Some(SyncAction::RecordSynced {
                    relative_path: path.to_string(),
                })
            } else if !l.is_dir && !r.is_dir && l.sha1 == r.sha1 {
                // Same content — just record.
                Some(SyncAction::RecordSynced {
                    relative_path: path.to_string(),
                })
            } else {
                // Different content on both sides — conflict.
                Some(SyncAction::Conflict {
                    relative_path: path.to_string(),
                    box_id: r.box_id.clone(),
                })
            }
        }

        // --- Exists only locally (new local file) ---
        (Some(l), None, None) => {
            if l.is_dir {
                // Need to find the parent folder's Box ID.
                // The sync engine will resolve this using the DB.
                Some(SyncAction::CreateRemoteDir {
                    relative_path: path.to_string(),
                    parent_id: String::new(), // resolved by sync engine
                })
            } else {
                Some(SyncAction::Upload {
                    relative_path: path.to_string(),
                    parent_id: String::new(), // resolved by sync engine
                })
            }
        }

        // --- Exists only remotely (new remote file) ---
        (None, Some(r), None) => {
            if r.is_dir {
                Some(SyncAction::CreateLocalDir {
                    relative_path: path.to_string(),
                })
            } else {
                Some(SyncAction::Download {
                    relative_path: path.to_string(),
                    box_id: r.box_id.clone(),
                })
            }
        }

        // --- In DB but deleted locally ---
        (None, Some(r), Some(_d)) => {
            // Deleted locally, still on remote.
            // Check if remote changed since last sync.
            // For simplicity: if remote SHA1 changed, download (remote wins over local delete).
            // Otherwise, delete on remote.
            if !r.is_dir && r.sha1.as_deref() != _d.remote_sha1.as_deref() {
                // Remote was modified — remote wins, re-download.
                Some(SyncAction::Download {
                    relative_path: path.to_string(),
                    box_id: r.box_id.clone(),
                })
            } else {
                // Remote unchanged — propagate the delete.
                Some(SyncAction::DeleteRemote {
                    box_id: r.box_id.clone(),
                    etag: r.etag.clone(),
                    is_dir: r.is_dir,
                })
            }
        }

        // --- In DB but deleted remotely ---
        (Some(l), None, Some(d)) => {
            // Deleted remotely, still local.
            if !l.is_dir && l.sha1.as_deref() != d.local_sha1.as_deref() {
                // Local was modified — local wins, re-upload.
                Some(SyncAction::Upload {
                    relative_path: path.to_string(),
                    parent_id: d.box_parent_id.clone().unwrap_or_default(),
                })
            } else {
                // Local unchanged — propagate the delete.
                Some(SyncAction::DeleteLocal {
                    relative_path: path.to_string(),
                })
            }
        }

        // --- In DB but deleted on both sides ---
        (None, None, Some(d)) => Some(SyncAction::RemoveDbEntry { entry_id: d.id }),

        // --- No info at all (shouldn't happen) ---
        (None, None, None) => None,
    }
}

/// Three-way comparison: entry exists in local, remote, AND DB.
fn reconcile_three_way(
    path: &str,
    local: &LocalEntry,
    remote: &RemoteEntry,
    db: &SyncEntry,
) -> Option<SyncAction> {
    // Directories don't have content to compare — just ensure they exist.
    if local.is_dir && remote.is_dir {
        return None; // Both exist, nothing to do.
    }

    let local_changed = local.sha1.as_deref() != db.local_sha1.as_deref();
    let remote_changed = remote.sha1.as_deref() != db.remote_sha1.as_deref();

    match (local_changed, remote_changed) {
        // Neither changed — in sync.
        (false, false) => None,

        // Only local changed — upload.
        (true, false) => Some(SyncAction::UploadVersion {
            relative_path: path.to_string(),
            file_id: remote.box_id.clone(),
            etag: remote.etag.clone(),
        }),

        // Only remote changed — download.
        (false, true) => Some(SyncAction::Download {
            relative_path: path.to_string(),
            box_id: remote.box_id.clone(),
        }),

        // Both changed — conflict.
        (true, true) => {
            // If they happen to have the same content now, no conflict.
            if local.sha1 == remote.sha1 {
                Some(SyncAction::RecordSynced {
                    relative_path: path.to_string(),
                })
            } else {
                Some(SyncAction::Conflict {
                    relative_path: path.to_string(),
                    box_id: remote.box_id.clone(),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_file(path: &str, sha1: &str) -> LocalEntry {
        LocalEntry {
            relative_path: path.to_string(),
            is_dir: false,
            sha1: Some(sha1.to_string()),
            size: 100,
            modified_at: "2025-01-01T00:00:00Z".to_string(),
        }
    }

    fn remote_file(path: &str, sha1: &str, box_id: &str) -> RemoteEntry {
        RemoteEntry {
            relative_path: path.to_string(),
            is_dir: false,
            box_id: box_id.to_string(),
            parent_id: "0".to_string(),
            sha1: Some(sha1.to_string()),
            etag: Some("etag1".to_string()),
            size: 100,
            modified_at: Some("2025-01-01T00:00:00Z".to_string()),
        }
    }

    fn db_entry(path: &str, local_sha1: &str, remote_sha1: &str, box_id: &str) -> SyncEntry {
        SyncEntry {
            id: 1,
            sync_root_index: 0,
            box_id: Some(box_id.to_string()),
            box_parent_id: Some("0".to_string()),
            entry_type: "file".to_string(),
            relative_path: path.to_string(),
            local_sha1: Some(local_sha1.to_string()),
            remote_sha1: Some(remote_sha1.to_string()),
            remote_etag: Some("etag1".to_string()),
            remote_version_id: None,
            remote_modified_at: None,
            local_modified_at: None,
            local_size: Some(100),
            sync_status: "synced".to_string(),
            last_sync_at: None,
            last_error: None,
            retry_count: 0,
            trashed_at: None,
            trash_path: None,
        }
    }

    #[test]
    fn test_no_changes() {
        let local = vec![local_file("a.txt", "aaa")];
        let remote = vec![remote_file("a.txt", "aaa", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert!(actions.is_empty(), "expected no actions, got: {actions:?}");
    }

    #[test]
    fn test_local_modified() {
        let local = vec![local_file("a.txt", "bbb")];
        let remote = vec![remote_file("a.txt", "aaa", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], SyncAction::UploadVersion { relative_path, .. } if relative_path == "a.txt")
        );
    }

    #[test]
    fn test_remote_modified() {
        let local = vec![local_file("a.txt", "aaa")];
        let remote = vec![remote_file("a.txt", "ccc", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], SyncAction::Download { relative_path, .. } if relative_path == "a.txt")
        );
    }

    #[test]
    fn test_conflict() {
        let local = vec![local_file("a.txt", "bbb")];
        let remote = vec![remote_file("a.txt", "ccc", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], SyncAction::Conflict { relative_path, .. } if relative_path == "a.txt")
        );
    }

    #[test]
    fn test_conflict_same_content() {
        // Both sides changed to the same content — no conflict.
        let local = vec![local_file("a.txt", "bbb")];
        let remote = vec![remote_file("a.txt", "bbb", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::RecordSynced { .. }));
    }

    #[test]
    fn test_new_local_file() {
        let local = vec![local_file("new.txt", "xxx")];
        let remote = vec![];
        let db = vec![];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], SyncAction::Upload { relative_path, .. } if relative_path == "new.txt")
        );
    }

    #[test]
    fn test_new_remote_file() {
        let local = vec![];
        let remote = vec![remote_file("new.txt", "xxx", "f2")];
        let db = vec![];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(
            matches!(&actions[0], SyncAction::Download { relative_path, .. } if relative_path == "new.txt")
        );
    }

    #[test]
    fn test_deleted_locally_remote_unchanged() {
        let local = vec![];
        let remote = vec![remote_file("a.txt", "aaa", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::DeleteRemote { .. }));
    }

    #[test]
    fn test_deleted_locally_remote_changed() {
        // Local deleted, but remote was updated — remote wins.
        let local = vec![];
        let remote = vec![remote_file("a.txt", "ccc", "f1")];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::Download { .. }));
    }

    #[test]
    fn test_deleted_remotely_local_unchanged() {
        let local = vec![local_file("a.txt", "aaa")];
        let remote = vec![];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::DeleteLocal { .. }));
    }

    #[test]
    fn test_deleted_remotely_local_changed() {
        // Remote deleted, but local was modified — local wins.
        let local = vec![local_file("a.txt", "bbb")];
        let remote = vec![];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::Upload { .. }));
    }

    #[test]
    fn test_deleted_both_sides() {
        let local = vec![];
        let remote = vec![];
        let db = vec![db_entry("a.txt", "aaa", "aaa", "f1")];

        let actions = reconcile(&local, &remote, &db);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            SyncAction::RemoveDbEntry { entry_id: 1 }
        ));
    }
}
