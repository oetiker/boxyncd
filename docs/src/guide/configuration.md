# Configuration

boxyncd reads its configuration from `~/.config/boxyncd/config.toml`.

If no config file exists, boxyncd will offer to create one from a built-in template.

## Config File

The minimal config only needs sync roots — official release builds ship with
built-in Box app credentials, so the `[auth]` section is optional:

### Example

```toml
[general]
full_sync_interval_secs = 300
local_debounce_ms = 2000
max_concurrent_transfers = 4

[[sync]]
local_path = "/home/user/BoxSync"
box_folder_id = "0"
exclude = ["*.tmp", ".git/**", "node_modules/**"]
remote_trash_path = ".trash"
remote_trash_days = 30
```

### Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `general.full_sync_interval_secs` | 86400 | Full reconciliation safety net (seconds, default: 1 day) |
| `general.local_debounce_ms` | 2000 | Debounce delay for local filesystem events |
| `general.max_concurrent_transfers` | 4 | Maximum concurrent uploads/downloads |
| `general.db_path` | auto | Custom path for the SQLite database file |
| `auth.client_id` | *built-in* | Box app client ID (optional with release builds) |
| `auth.client_secret` | *built-in* | Box app client secret (optional with release builds) |
| `auth.redirect_port` | 8080 | Local OAuth callback server port |
| `auth.token_path` | auto | Custom token storage path |

### Sync Roots

Each `[[sync]]` block maps a local directory to a Box folder:

- `local_path` - Absolute path to local directory
- `box_folder_id` - Box folder ID (`"0"` for account root)
- `exclude` - Glob patterns for files to ignore
- `remote_trash_path` - Directory for trashed files (optional, see below)
- `remote_trash_days` - Days to keep trashed files before permanent deletion (default: 30)

You can define multiple sync roots to sync different folder pairs.

### Local Trash Bin

When a file is moved to the Box.com Trash Bin (i.e. deleted in the Box web UI or app), boxyncd normally deletes the local copy too. By setting `remote_trash_path`, you can have boxyncd move those files into a local trash directory instead. This mirrors Box.com's own Trash Bin but on the local side.

- Only **remote-initiated deletions** are affected — files you delete locally still propagate to Box normally.
- The path can be absolute or relative to `local_path`. When it falls inside the sync tree, it is automatically excluded from syncing.
- Trashed files are organized in date-stamped subdirectories (e.g. `.trash/2026-02-19/report.pdf`).
- After `remote_trash_days` (default 30), trashed files are permanently deleted at the end of the next sync cycle.

### Custom Box App (optional)

If you want to use your own Box developer app instead of the built-in one:

1. Go to [Box Developer Console](https://app.box.com/developers/console)
2. Create a new **Custom App** with **User Authentication (OAuth 2.0)**
3. Under **Configuration**, add redirect URI: `http://127.0.0.1:8080/callback`
4. Add an `[auth]` section to your config with your Client ID and Client Secret
