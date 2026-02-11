# boxyncd

[![CI](https://github.com/oetiker/boxyncd/actions/workflows/ci.yml/badge.svg)](https://github.com/oetiker/boxyncd/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/oetiker/boxyncd)](https://github.com/oetiker/boxyncd/releases/latest)
[![License](https://img.shields.io/github/license/oetiker/boxyncd)](LICENSE)

Bidirectional Box.com file sync daemon for Linux.

boxyncd keeps local directories in sync with Box.com cloud storage using a three-way merge algorithm. Changes on either side are detected and propagated automatically, with conflict handling when both sides change.

## Quick Start

1. Download the latest binary from [GitHub Releases](https://github.com/oetiker/boxyncd/releases)
2. Copy it to `~/.local/bin/boxyncd` and make it executable
3. Create a config file at `~/.config/boxyncd/config.toml` (see [example](config/boxyncd.example.toml))
4. Authenticate: `boxyncd auth`
5. Install the systemd service: `boxyncd service install`

## Features

- **Bidirectional sync** between local directories and Box.com folders
- **Three-way merge** using a database baseline to detect conflicts
- **Real-time monitoring** via inotify (local) and Box event stream (remote)
- **Conflict resolution** with automatic conflict copy creation
- **Multiple sync roots** for mapping different folder pairs
- **Systemd integration** via `boxyncd service install/uninstall/status/log`

## Documentation

Full documentation is available at **[oetiker.github.io/boxyncd](https://oetiker.github.io/boxyncd)**:

- [Installation Guide](https://oetiker.github.io/boxyncd/dev/guide/installation.html)
- [Configuration](https://oetiker.github.io/boxyncd/dev/guide/configuration.html)
- [Usage](https://oetiker.github.io/boxyncd/dev/guide/usage.html)

## Building from Source

Requires Rust 2024 edition (1.85+).

```bash
git clone https://github.com/oetiker/boxyncd.git
cd boxyncd
make release
cp target/release/boxyncd ~/.local/bin/
```

## License

MIT License - see [LICENSE](LICENSE)
