use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Built-in Box app credentials, injected at compile time via environment variables.
/// Release builds set these via GitHub secrets; dev builds get empty strings.
const BUILTIN_CLIENT_ID: &str = match option_env!("BOX_CLIENT_ID") {
    Some(v) => v,
    None => "",
};
const BUILTIN_CLIENT_SECRET: &str = match option_env!("BOX_CLIENT_SECRET") {
    Some(v) => v,
    None => "",
};

fn default_client_id() -> String {
    BUILTIN_CLIENT_ID.to_string()
}
fn default_client_secret() -> String {
    BUILTIN_CLIENT_SECRET.to_string()
}

/// Returns true when the binary ships with built-in Box app credentials.
pub fn has_builtin_credentials() -> bool {
    !BUILTIN_CLIENT_ID.is_empty() && !BUILTIN_CLIENT_SECRET.is_empty()
}

const EXAMPLE_CONFIG: &str = include_str!("../config/boxyncd.example.toml");

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub sync: Vec<SyncRoot>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct GeneralConfig {
    #[serde(default = "default_full_sync_interval")]
    pub full_sync_interval_secs: u64,
    #[serde(default = "default_debounce_ms")]
    pub local_debounce_ms: u64,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_transfers: usize,
    /// Custom path for the SQLite database file
    pub db_path: Option<PathBuf>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            full_sync_interval_secs: default_full_sync_interval(),
            local_debounce_ms: default_debounce_ms(),
            log_level: default_log_level(),
            max_concurrent_transfers: default_max_concurrent(),
            db_path: None,
        }
    }
}

fn default_full_sync_interval() -> u64 {
    86400
}
fn default_debounce_ms() -> u64 {
    2000
}
fn default_log_level() -> String {
    "info".into()
}
fn default_max_concurrent() -> usize {
    4
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_client_id")]
    pub client_id: String,
    #[serde(default = "default_client_secret")]
    pub client_secret: String,
    /// Port for the local OAuth callback server
    pub redirect_port: Option<u16>,
    /// Custom path for token storage
    pub token_path: Option<PathBuf>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            client_id: default_client_id(),
            client_secret: default_client_secret(),
            redirect_port: None,
            token_path: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncRoot {
    pub local_path: PathBuf,
    pub box_folder_id: String,
    #[serde(default)]
    pub exclude: Vec<String>,
}

pub fn default_config_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("Could not determine config directory")?;
    Ok(dir.join("boxyncd").join("config.toml"))
}

/// Create a minimal Config suitable for `boxyncd auth` when no config file exists.
/// Uses built-in credentials (or empty strings for dev builds).
pub fn auth_only_config() -> Config {
    Config {
        general: GeneralConfig::default(),
        auth: AuthConfig::default(),
        sync: Vec::new(),
    }
}

/// Offer to create a default config file at `path`. Returns Ok(true) if created.
fn offer_create_config(path: &Path) -> Result<bool> {
    eprint!(
        "Config file not found at {}.\nCreate it with defaults? [Y/n] ",
        path.display()
    );
    std::io::stderr().flush()?;

    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();

    if !answer.is_empty() && !answer.eq_ignore_ascii_case("y") {
        eprintln!(
            "No config created. To get started, copy the example:\n  \
             mkdir -p {dir} && cp config/boxyncd.example.toml {path}",
            dir = path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            path = path.display(),
        );
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }
    std::fs::write(path, EXAMPLE_CONFIG)
        .with_context(|| format!("Failed to write config: {}", path.display()))?;

    eprintln!("Config written to {}", path.display());
    eprintln!("Edit it to add your sync roots, then run `boxyncd auth`.");
    Ok(true)
}

pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => default_config_path()?,
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if offer_create_config(&path)? {
                std::process::exit(0);
            }
            anyhow::bail!("No config file — cannot continue.");
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read config file: {}", path.display()));
        }
    };

    let config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

    // Only require credentials in the config when no built-in values are available
    if config.auth.client_id.is_empty() && !has_builtin_credentials() {
        anyhow::bail!(
            "auth.client_id is empty and no built-in credentials are available.\n\
             Set client_id/client_secret in [auth] or use an official release build."
        );
    }
    if config.auth.client_secret.is_empty() && !has_builtin_credentials() {
        anyhow::bail!(
            "auth.client_secret is empty and no built-in credentials are available.\n\
             Set client_id/client_secret in [auth] or use an official release build."
        );
    }

    Ok(config)
}
