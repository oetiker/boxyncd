# Installation

## Quick Start

1. Download the latest `boxyncd` binary from the
   [GitHub releases page](https://github.com/oetiker/boxyncd/releases)
   and place it somewhere in your `PATH`:

   ```bash
   cp boxyncd ~/.local/bin/
   chmod +x ~/.local/bin/boxyncd
   ```

2. Authenticate with Box.com:

   ```bash
   boxyncd auth
   ```

3. Configure your sync roots in `~/.config/boxyncd/config.toml`
   (boxyncd will offer to create the file if it doesn't exist).

4. Start syncing:

   ```bash
   boxyncd start
   ```

## Systemd Service

To run boxyncd as a systemd user service:

```bash
boxyncd service install
```

This will:

1. Write a unit file to `~/.config/systemd/user/boxyncd.service`
2. Run `systemctl --user daemon-reload`
3. Enable and start the service

To remove the service:

```bash
boxyncd service uninstall
```

View logs:

```bash
boxyncd service log
```

### Lingering

For boxyncd to start at boot and keep running after you log out, enable lingering:

```bash
sudo loginctl enable-linger $USER
```
