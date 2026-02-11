-- Tracks the state of every known file and folder across all sync roots.
-- This is the "baseline" for three-way comparison: what both sides looked
-- like after the last successful sync.

CREATE TABLE IF NOT EXISTS sync_entries (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_root_index     INTEGER NOT NULL,
    box_id              TEXT,
    box_parent_id       TEXT,
    entry_type          TEXT NOT NULL CHECK(entry_type IN ('file', 'folder')),
    relative_path       TEXT NOT NULL,
    local_sha1          TEXT,
    remote_sha1         TEXT,
    remote_etag         TEXT,
    remote_version_id   TEXT,
    remote_modified_at  TEXT,
    local_modified_at   TEXT,
    local_size          INTEGER,
    sync_status         TEXT NOT NULL DEFAULT 'synced'
                        CHECK(sync_status IN (
                            'synced',
                            'local_modified',
                            'remote_modified',
                            'conflict',
                            'new_local',
                            'new_remote',
                            'deleted_local',
                            'deleted_remote',
                            'error'
                        )),
    last_sync_at        TEXT,
    last_error          TEXT,
    retry_count         INTEGER NOT NULL DEFAULT 0,
    UNIQUE(sync_root_index, relative_path)
);

CREATE INDEX IF NOT EXISTS idx_sync_entries_box_id
    ON sync_entries(box_id);
CREATE INDEX IF NOT EXISTS idx_sync_entries_status
    ON sync_entries(sync_status);
CREATE INDEX IF NOT EXISTS idx_sync_entries_root_path
    ON sync_entries(sync_root_index, relative_path);

-- Tracks the Box event stream position so we resume after restart.
CREATE TABLE IF NOT EXISTS stream_positions (
    sync_root_index     INTEGER PRIMARY KEY,
    stream_position     TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

-- Tracks in-flight operations for crash recovery.
CREATE TABLE IF NOT EXISTS pending_operations (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    sync_entry_id       INTEGER NOT NULL REFERENCES sync_entries(id),
    operation           TEXT NOT NULL CHECK(operation IN (
                            'upload', 'download', 'delete_local', 'delete_remote',
                            'mkdir_local', 'mkdir_remote', 'rename', 'move'
                        )),
    state               TEXT NOT NULL DEFAULT 'pending'
                        CHECK(state IN ('pending', 'in_progress', 'completed', 'failed')),
    metadata_json       TEXT,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
