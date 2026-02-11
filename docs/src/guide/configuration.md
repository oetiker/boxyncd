# Configuration

boxyncd reads its configuration from `~/.config/boxyncd/config.toml`.

## Box Developer App

Before using boxyncd, create a Box developer app:

1. Go to [Box Developer Console](https://app.box.com/developers/console)
2. Create a new **Custom App** with **User Authentication (OAuth 2.0)**
3. Under **Configuration**, add redirect URI: `http://127.0.0.1:8080/callback`
4. Note the **Client ID** and **Client Secret**

## Config File

Copy the example config and fill in your credentials:

```bash
mkdir -p ~/.config/boxyncd
cp config/boxyncd.example.toml ~/.config/boxyncd/config.toml
```

### Example

```toml
[general]
full_sync_interval_secs = 300
local_debounce_ms = 2000
max_concurrent_transfers = 4

[auth]
client_id = "your_client_id_here"
client_secret = "your_client_secret_here"

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
| `auth.client_id` | *required* | Box app client ID |
| `auth.client_secret` | *required* | Box app client secret |
| `auth.redirect_port` | 8080 | Local OAuth callback server port |
| `auth.token_path` | auto | Custom token storage path |

### Sync Roots

Each `[[sync]]` block maps a local directory to a Box folder:

- `local_path` - Absolute path to local directory
- `box_folder_id` - Box folder ID (`"0"` for account root)
- `exclude` - Glob patterns for files to ignore

You can define multiple sync roots to sync different folder pairs.
