# Changelog

## Unreleased

### New

### Changed

### Fixed

- Fix Box API rate-limit errors killing sync: retry up to 5 times with backoff instead of failing immediately

## 0.4.1 - 2026-02-16

### Fixed

- Fix apparent hang when syncing large folder hierarchies (add per-folder progress logging, parallelize remote tree walk)

## 0.4.0 - 2026-02-12

### New

- Add `boxyncd dry-run` subcommand: validates config, auth, Box connectivity, folder access, and previews sync actions without making changes
- Add `db_path` option to `[general]` config section for custom database file location
### Changed

### Fixed

## 0.3.1 - 2026-02-12

### New

- Added illumos amd64 (`x86_64-unknown-illumos`) binary to release builds
### Changed

### Fixed

- Documentation version selector now defaults to the latest stable release instead of the dev version

## 0.3.0 - 2026-02-11

### New

- Show build date in `--version` output
### Changed

- Incremental sync now operates on individual files instead of walking entire trees, reducing inotify noise and API calls
- Example config shows how to find a Box folder's ID from its URL
- Change default full sync interval from 5 minutes to 1 day (real-time watchers handle changes)

### Fixed

- Fix panic on filenames with Unicode combining characters (e.g. `ä` as `a` + combining umlaut)
- Fix upload failures caused by nanosecond-precision timestamps rejected by Box API
- Show full error chain in action failure logs
- Fix remote event watcher crash when Box returns stream position as number instead of string
- Fix sync feedback loop: exact path-based echo suppression instead of time-based cooldown

## 0.2.2 - 2026-02-11

### New

- Embed Box app credentials at compile time via `BOX_CLIENT_ID`/`BOX_CLIENT_SECRET` env vars
- `boxyncd auth` works without a config file when built-in credentials are available
- Auto-create config file from built-in template when missing (interactive prompt)
- Add `Cross.toml` to pass credentials through to cross-compilation containers

### Changed

- `[auth]` section in config is now optional — release builds use built-in credentials
- Simplified onboarding: download binary, run `boxyncd auth`, configure sync roots
- Example config no longer includes `client_id`/`client_secret` fields

### Fixed

## 0.2.1 - 2026-02-11

## 0.2.0 - 2026-02-11

### New

- Add `boxyncd service install/uninstall/status/log` subcommands for systemd user service management
- Auto-detect and set D-Bus environment variables so `systemctl --user` works in SSH sessions
- Warn when lingering is not enabled (needed for start-at-boot and surviving logout)
- Generate systemd unit file dynamically using the current binary path
### Changed

- Installation docs now point to GitHub Releases instead of compile-from-source instructions
- Document conflict resolution behavior: local keeps original filename, remote saved as timestamped conflict copy
- Add delete-vs-edit and delete-vs-delete edge cases to sync action table

### Fixed

- Fix `versions.json` writing string `"null"` instead of JSON `null` when no releases exist
- Fix `generate-redirect` script failing under `set -euo pipefail` when `latest` is JSON `null`

## 0.1.0 - 2026-02-11

### New

- Initial release
- Bidirectional Box.com file sync with three-way merge algorithm
- Real-time local monitoring via inotify and remote monitoring via Box event stream
- Conflict detection and resolution with timestamped conflict copies
- Multiple sync root support
- OAuth authentication flow
- Versioned documentation deployment to GitHub Pages
- CI/CD with binary-only releases for x86_64 and aarch64 Linux
