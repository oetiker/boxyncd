-- Add local trash bin support: track trashed files for delayed cleanup.
-- trashed_at: when the file was moved to trash (NULL = not trashed)
-- trash_path: absolute filesystem path where the trashed file now lives
ALTER TABLE sync_entries ADD COLUMN trashed_at TEXT;
ALTER TABLE sync_entries ADD COLUMN trash_path TEXT;
