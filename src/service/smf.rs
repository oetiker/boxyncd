use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

const FMRI: &str = "svc:/application/boxyncd:default";

fn manifest_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(PathBuf::from(home).join(".config/boxyncd/smf"))
}

fn manifest_path() -> Result<PathBuf> {
    Ok(manifest_dir()?.join("boxyncd.xml"))
}

fn generate_manifest(exe_path: &std::path::Path, config_path: Option<&std::path::Path>) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let exec_start = match config_path {
        Some(cfg) => format!("{} -c {} start", exe_path.display(), cfg.display()),
        None => format!("{} start", exe_path.display()),
    };
    format!(
        r#"<?xml version="1.0"?>
<!DOCTYPE service_bundle SYSTEM "/usr/share/lib/xml/dtd/service_bundle.dtd.1">
<service_bundle type="manifest" name="boxyncd">
  <service name="application/boxyncd" type="service" version="1">
    <create_default_instance enabled="false"/>
    <single_instance/>

    <dependency name="network" grouping="require_all" restart_on="error" type="service">
      <service_fmri value="svc:/milestone/network:default"/>
    </dependency>

    <exec_method type="method" name="start"
      exec="{exec_start}"
      timeout_seconds="60">
      <method_context>
        <method_environment>
          <envvar name="HOME" value="{home}"/>
          <envvar name="RUST_LOG" value="boxyncd=info"/>
        </method_environment>
      </method_context>
    </exec_method>

    <exec_method type="method" name="stop" exec=":kill" timeout_seconds="30"/>

    <property_group name="startd" type="framework">
      <propval name="duration" type="astring" value="child"/>
    </property_group>

    <stability value="Evolving"/>

    <template>
      <common_name>
        <loctext xml:lang="C">Bidirectional Box.com file sync daemon</loctext>
      </common_name>
    </template>
  </service>
</service_bundle>
"#,
        home = home,
    )
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

pub fn install(config_path: Option<&std::path::Path>) -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine path to current executable")?;
    let manifest = generate_manifest(&exe, config_path);

    let dir = manifest_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let path = manifest_path()?;
    std::fs::write(&path, &manifest)
        .with_context(|| format!("failed to write {}", path.display()))?;
    println!("Wrote {}", path.display());

    run_cmd(
        Command::new("svccfg").arg("import").arg(&path),
        "svccfg import",
    )?;
    run_cmd(
        Command::new("svcadm").arg("enable").arg(FMRI),
        "svcadm enable",
    )?;

    println!();
    println!("boxyncd service installed and started.");
    println!("View logs with: boxyncd service log");

    Ok(())
}

pub fn uninstall() -> Result<()> {
    // Disable synchronously (ignore errors if not running/enabled)
    let _ = Command::new("svcadm")
        .args(["disable", "-s", FMRI])
        .status();
    let _ = Command::new("svccfg").args(["delete", FMRI]).status();

    let path = manifest_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove {}", path.display()))?;
        println!("Removed {}", path.display());
    }

    println!("boxyncd service uninstalled.");
    Ok(())
}

pub fn status() -> Result<()> {
    // svcs returns non-zero when the service is not found or in maintenance,
    // so we don't treat that as an error — just forward the output.
    let _ = Command::new("svcs").args(["-l", FMRI]).status();
    Ok(())
}

pub fn log() -> Result<()> {
    // Ask svcs for the log file path
    let output = Command::new("svcs")
        .args(["-L", FMRI])
        .output()
        .context("failed to run svcs -L")?;

    if !output.status.success() {
        anyhow::bail!(
            "svcs -L failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let log_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if log_path.is_empty() {
        anyhow::bail!("svcs -L returned empty log path");
    }

    let err = Command::new("tail").args(["-f", &log_path]).exec();
    // CommandExt::exec() only returns on error
    Err(err).context("failed to exec tail -f")
}
