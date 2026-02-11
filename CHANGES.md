# Changelog

## Unreleased

### New

### Changed

### Fixed

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
