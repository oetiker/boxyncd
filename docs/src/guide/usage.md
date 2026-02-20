# Usage

## Commands

### Authenticate

```bash
boxyncd auth
```

Opens your browser for Box.com OAuth authentication. Release builds include
built-in Box app credentials, so this works out of the box — no developer app
setup required. Tokens are stored locally at `~/.local/share/boxyncd/tokens.json`.

### Start Sync Daemon

```bash
boxyncd start
```

Starts the sync daemon in the foreground. It will:

1. Run an initial full sync
2. Watch for local filesystem changes (inotify)
3. Watch for remote changes (Box event stream)
4. Run periodic full syncs at the configured interval

Use `-v` for debug output or `-vv` for trace output:

```bash
boxyncd -v start
```

### Check Status

```bash
boxyncd status
```

Shows a summary of tracked files, sync roots, errors, and last sync times.

### Trigger Immediate Sync

```bash
boxyncd sync-now
```

Runs a single full sync cycle and exits.

### Dry Run

```bash
boxyncd dry-run
```

Validates your setup without making any changes:

- Parses and validates the config file
- Checks that each sync root's local directory exists and is writable
- Verifies the auth token can be loaded
- Tests Box API connectivity (calls `/users/me`)
- Checks access to each configured Box folder
- Previews what sync actions would be taken (walks both trees and runs the reconciler read-only)

Exits with code 0 if everything passes and there is nothing to sync (or only shows pending actions), or code 1 if any check fails.

### Manage Systemd Service

```bash
boxyncd service install      # Install, enable, and start the user service
boxyncd service uninstall    # Stop, disable, and remove the user service
boxyncd service status       # Show systemctl status
boxyncd service log          # Tail service logs (journalctl -f)
```

The `install` command generates a systemd unit file using the path of the current binary and writes it to `~/.config/systemd/user/boxyncd.service`. If lingering is not enabled for your user account, it will print a warning with the command to enable it.

## How Sync Works

boxyncd uses a **three-way merge** algorithm:

1. **Local state** - Files on your local disk (SHA-1 hashed)
2. **Remote state** - Files on Box.com (SHA-1 from API)
3. **Baseline state** - Last known synced state (stored in SQLite)

By comparing all three, boxyncd determines the correct action for each file:

| Local | Remote | Baseline | Action |
|-------|--------|----------|--------|
| Changed | Unchanged | Known | Upload |
| Unchanged | Changed | Known | Download |
| Changed | Changed (different) | Known | Conflict |
| Changed | Changed (same) | Known | Record synced |
| New | Missing | Missing | Upload |
| Missing | New | Missing | Download |
| Deleted | Unchanged | Known | Trash remote |
| Deleted | Changed | Known | Download (remote wins) |
| Unchanged | Trashed / Deleted | Known | Delete local (or move to trash) |
| Changed | Trashed / Deleted | Known | Upload (local wins) |
| Deleted | Trashed / Deleted | Known | Remove from DB |

**Note:** "Trash remote" means the file is moved to the Box Trash Bin via the Box API — it is not permanently deleted. You can restore it from the Box Trash if needed. Similarly, "Trashed / Deleted" on the remote side means Box reports the file as trashed (e.g. deleted in the Box web UI or app).

### Local Trash Bin

When you delete a file in the Box web UI or app, Box moves it to its own Trash Bin (it is not permanently gone). boxyncd detects this trashed status and by default deletes the local copy. When `remote_trash_path` is configured on a sync root, the local copy is moved into the local trash directory instead of being permanently deleted. Files are kept for `remote_trash_days` (default 30) and then cleaned up automatically. See [Configuration](configuration.md#local-trash-bin) for details.

### Timestamp Syncing

boxyncd preserves file timestamps across both sides. Downloaded files keep the remote `content_modified_at` as their local modification time. On the first sync after upgrading to v0.6.0+, boxyncd consolidates timestamps: for each file it picks the older timestamp and pushes it to whichever side is newer, so both local and remote end up with a consistent modification time.

### Conflict Resolution

When both sides change a file differently, boxyncd keeps your local version under the original filename and downloads the remote version as a timestamped conflict copy, e.g.:

- `report.pdf` → `report (Conflicted Copy 2026-02-11 10-30-45).pdf`
- `Makefile` → `Makefile (Conflicted Copy 2026-02-11 10-30-45)`

If both sides happen to change to the same content (identical SHA-1), no conflict is raised.

You can resolve conflicts by comparing the two files and deleting the one you don't need. On the next sync cycle the remaining file will be uploaded to Box.
