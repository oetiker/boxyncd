# boxyncd

Bidirectional Box.com file sync daemon for Linux.

boxyncd keeps local directories in sync with Box.com cloud storage using a three-way merge algorithm. Changes on either side are detected and propagated automatically, with conflict handling when both sides change.

## Features

- **Bidirectional sync** between local directories and Box.com folders
- **Three-way merge** using a database baseline to detect conflicts
- **Real-time monitoring** via inotify (local) and Box event stream (remote)
- **Conflict resolution** with automatic conflict copy creation
- **Multiple sync roots** for mapping different folder pairs
- **Systemd integration** for running as a user service

## Quick Start

1. [Install boxyncd](guide/installation.md)
2. [Create a Box developer app and configure](guide/configuration.md)
3. Authenticate: `boxyncd auth`
4. Start syncing: `boxyncd start`

See the [Usage guide](guide/usage.md) for detailed command reference.
