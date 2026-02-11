use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

pub fn resolve_token_path(custom: Option<&Path>) -> Result<PathBuf> {
    match custom {
        Some(p) => Ok(p.to_path_buf()),
        None => {
            let dir = dirs::data_dir().context("Could not determine data directory")?;
            Ok(dir.join("boxyncd").join("tokens.json"))
        }
    }
}

pub fn load_tokens(path: &Path) -> Result<TokenData> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read token file: {}", path.display()))?;
    let tokens: TokenData = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse token file: {}", path.display()))?;
    Ok(tokens)
}

pub fn save_tokens(path: &Path, tokens: &TokenData) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(tokens)?;

    // Atomic write: tmp file → rename
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .with_context(|| format!("Failed to write token file: {}", tmp.display()))?;

    // Restrict permissions to owner-only (0600) before renaming into place
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }

    std::fs::rename(&tmp, path)
        .with_context(|| format!("Failed to save token file: {}", path.display()))?;

    Ok(())
}
