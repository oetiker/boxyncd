use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

fn get_uid() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(0)
}

/// Ensure XDG_RUNTIME_DIR and DBUS_SESSION_BUS_ADDRESS are set so that
/// `systemctl --user` and `journalctl --user` work in bare SSH sessions.
fn ensure_user_systemd_env(cmd: &mut Command) {
    let uid = get_uid();
    if std::env::var("XDG_RUNTIME_DIR").is_err() {
        cmd.env("XDG_RUNTIME_DIR", format!("/run/user/{uid}"));
    }
    if std::env::var("DBUS_SESSION_BUS_ADDRESS").is_err() {
        cmd.env(
            "DBUS_SESSION_BUS_ADDRESS",
            format!("unix:path=/run/user/{uid}/bus"),
        );
    }
}

fn systemctl(args: &[&str]) -> Command {
    let mut cmd = Command::new("systemctl");
    cmd.arg("--user");
    cmd.args(args);
    ensure_user_systemd_env(&mut cmd);
    cmd
}

fn journalctl(args: &[&str]) -> Command {
    let mut cmd = Command::new("journalctl");
    cmd.arg("--user");
    cmd.args(args);
    ensure_user_systemd_env(&mut cmd);
    cmd
}

fn service_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

fn service_file_path() -> Result<PathBuf> {
    Ok(service_dir()?.join("boxyncd.service"))
}

fn generate_unit_file(exe_path: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Bidirectional Box.com file sync daemon\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} start\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         KillSignal=SIGTERM\n\
         TimeoutStopSec=30\n\
         Environment=RUST_LOG=boxyncd=info\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = exe_path.display()
    )
}

fn get_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| format!("{}", get_uid()))
}

fn check_linger() {
    let username = get_username();
    let linger_path = format!("/var/lib/systemd/linger/{username}");
    if !Path::new(&linger_path).exists() {
        eprintln!();
        eprintln!("Warning: Lingering is not enabled for your user account.");
        eprintln!("Without lingering, the service will stop when you log out.");
        eprintln!("Enable it with: sudo loginctl enable-linger {username}");
    }
}

fn run_cmd(cmd: &mut Command, description: &str) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("failed to run: {description}"))?;
    if !status.success() {
        anyhow::bail!("{description} failed with exit code {}", status);
    }
    Ok(())
}

pub fn install() -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine path to current executable")?;
    let unit = generate_unit_file(&exe);

    let dir = service_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let path = service_file_path()?;
    std::fs::write(&path, &unit).with_context(|| format!("failed to write {}", path.display()))?;
    println!("Wrote {}", path.display());

    run_cmd(
        &mut systemctl(&["daemon-reload"]),
        "systemctl daemon-reload",
    )?;
    run_cmd(
        &mut systemctl(&["enable", "boxyncd"]),
        "systemctl enable boxyncd",
    )?;
    run_cmd(
        &mut systemctl(&["start", "boxyncd"]),
        "systemctl start boxyncd",
    )?;

    println!();
    println!("boxyncd service installed and started.");
    println!("View logs with: boxyncd service log");

    check_linger();

    Ok(())
}

pub fn uninstall() -> Result<()> {
    // Stop and disable (ignore errors if not running/enabled)
    let _ = systemctl(&["stop", "boxyncd"]).status();
    let _ = systemctl(&["disable", "boxyncd"]).status();

    let path = service_file_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        println!("Removed {}", path.display());
    }

    run_cmd(
        &mut systemctl(&["daemon-reload"]),
        "systemctl daemon-reload",
    )?;

    println!("boxyncd service uninstalled.");
    Ok(())
}

pub fn status() -> Result<()> {
    // systemctl status returns non-zero when the service is inactive, so
    // we don't treat that as an error — just forward the output.
    let _ = systemctl(&["status", "boxyncd"]).status();
    Ok(())
}

pub fn log() -> Result<()> {
    let err = journalctl(&["-u", "boxyncd", "-f"]).exec();
    // CommandExt::exec() only returns on error
    Err(err).context("failed to exec journalctl")
}
