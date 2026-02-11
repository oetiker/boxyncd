# CLAUDE.md — boxyncd project notes

## Project

Bidirectional Box.com file sync daemon for Linux. Rust 2024 edition (1.85+).

## Build & verify

```bash
cargo fmt               # format code
cargo clippy -- -D warnings  # lint (zero warnings policy)
cargo test              # run all tests
cargo build             # debug build
make release            # release build (runs fmt + clippy first)
```

Always run `cargo clippy -- -D warnings` and `cargo test` before committing.

## Git workflow

The release workflow (`gh workflow_dispatch`) bumps versions and pushes tags automatically.
This means the remote may have commits you don't have locally.

**Always `git pull --rebase` before pushing.** The CI may have pushed release commits or tag-triggered doc deployments.

## Architecture

- `src/main.rs` — CLI entry point (clap), daemon event loop
- `src/service.rs` — `boxyncd service install/uninstall/status/log` (systemd integration, no config needed)
- `src/auth/` — OAuth flow, token management
- `src/box_api/` — Box.com REST API client
- `src/sync/` — three-way merge engine, reconciler, local/remote watchers, up/downloaders
- `src/config.rs` — TOML config loading
- `src/db.rs` — SQLite via sqlx
- `docs/` — mdBook documentation (versioned deployment to GitHub Pages)

## Key design decisions

- `Service` commands are handled before `config::load_config()` — they don't need Box credentials
- `service.rs` sets `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` so `systemctl --user` works in SSH sessions
- `boxyncd service log` execs into journalctl (replaces the process) via `CommandExt::exec()`
- Conflict resolution: local keeps the original filename, remote is saved as a timestamped conflict copy
- Zero external dependencies for systemd integration — uses `std::process::Command` and `std::os::unix`

## GitHub Actions

- `ci.yml` — fmt, clippy, test on push/PR to main
- `docs.yml` — builds mdBook, deploys versioned docs (dev on push to main, versioned on tag push)
- `release.yml` — binary-only releases (workflow_dispatch), no doc deployment
