use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
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
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            full_sync_interval_secs: default_full_sync_interval(),
            local_debounce_ms: default_debounce_ms(),
            log_level: default_log_level(),
            max_concurrent_transfers: default_max_concurrent(),
        }
    }
}

fn default_full_sync_interval() -> u64 {
    300
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
    pub client_id: String,
    pub client_secret: String,
    /// Port for the local OAuth callback server
    pub redirect_port: Option<u16>,
    /// Custom path for token storage
    pub token_path: Option<PathBuf>,
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

pub fn load_config(path: Option<&Path>) -> Result<Config> {
    let path = match path {
        Some(p) => p.to_path_buf(),
        None => default_config_path()?,
    };

    let content = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "Failed to read config file: {}\n\
             Create it with your Box app credentials.\n\
             See config/boxyncd.example.toml for an example.",
            path.display()
        )
    })?;

    let config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

    if config.auth.client_id.is_empty() {
        anyhow::bail!("auth.client_id must not be empty");
    }
    if config.auth.client_secret.is_empty() {
        anyhow::bail!("auth.client_secret must not be empty");
    }

    Ok(config)
}
