# Usage

## Commands

### Authenticate

```bash
boxyncd auth
```

Opens your browser for Box.com OAuth authentication. Tokens are stored locally at `~/.local/share/boxyncd/tokens.json`.

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
| Deleted | Unchanged | Known | Delete remote |
| Unchanged | Deleted | Known | Delete local |

### Conflict Resolution

When both sides change a file differently, boxyncd creates a conflict copy with a timestamped name (e.g., `report (conflict 2025-02-11).pdf`) and downloads the remote version.
