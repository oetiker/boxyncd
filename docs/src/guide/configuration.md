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
```

### Settings

| Setting | Default | Description |
|---------|---------|-------------|
| `general.full_sync_interval_secs` | 300 | Full reconciliation interval (seconds) |
| `general.local_debounce_ms` | 2000 | Debounce delay for local filesystem events |
| `general.max_concurrent_transfers` | 4 | Maximum concurrent uploads/downloads |
| `auth.client_id` | *built-in* | Box app client ID (optional with release builds) |
| `auth.client_secret` | *built-in* | Box app client secret (optional with release builds) |
| `auth.redirect_port` | 8080 | Local OAuth callback server port |
| `auth.token_path` | auto | Custom token storage path |

### Sync Roots

Each `[[sync]]` block maps a local directory to a Box folder:

- `local_path` - Absolute path to local directory
- `box_folder_id` - Box folder ID (`"0"` for account root)
- `exclude` - Glob patterns for files to ignore

You can define multiple sync roots to sync different folder pairs.

### Custom Box App (optional)

If you want to use your own Box developer app instead of the built-in one:

1. Go to [Box Developer Console](https://app.box.com/developers/console)
2. Create a new **Custom App** with **User Authentication (OAuth 2.0)**
3. Under **Configuration**, add redirect URI: `http://127.0.0.1:8080/callback`
4. Add an `[auth]` section to your config with your Client ID and Client Secret
