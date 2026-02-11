mod oauth;
mod token_store;

pub use oauth::run_auth_flow;
pub use token_store::TokenData;

use std::path::PathBuf;

use anyhow::{Context, Result};
use tokio::sync::Mutex;

use crate::config::Config;

pub struct TokenManager {
    client_id: String,
    client_secret: String,
    token_path: PathBuf,
    tokens: Mutex<Option<TokenData>>,
    http_client: reqwest::Client,
}

impl TokenManager {
    pub fn new(config: &Config) -> Result<Self> {
        let token_path = token_store::resolve_token_path(config.auth.token_path.as_deref())?;
        let tokens = if token_path.exists() {
            match token_store::load_tokens(&token_path) {
                Ok(t) => {
                    tracing::info!("loaded existing tokens");
                    Some(t)
                }
                Err(e) => {
                    tracing::warn!("failed to load tokens: {e:#}");
                    None
                }
            }
        } else {
            None
        };

        if tokens.is_none() {
            tracing::warn!("not authenticated — run `boxyncd auth` first");
        }

        Ok(Self {
            client_id: config.auth.client_id.clone(),
            client_secret: config.auth.client_secret.clone(),
            token_path,
            tokens: Mutex::new(tokens),
            http_client: reqwest::Client::new(),
        })
    }

    /// Get a valid access token, refreshing if necessary.
    pub async fn get_access_token(&self) -> Result<String> {
        let mut guard = self.tokens.lock().await;
        let tokens = guard
            .as_mut()
            .context("Not authenticated. Run `boxyncd auth` first.")?;

        // Refresh if token expires within 60 seconds
        let now = chrono::Utc::now();
        let buffer = chrono::Duration::seconds(60);
        if tokens.expires_at <= now + buffer {
            tracing::debug!("access token expired or expiring soon, refreshing");
            let refreshed = self.refresh(tokens).await?;
            *tokens = refreshed;
            token_store::save_tokens(&self.token_path, tokens)?;
            tracing::debug!("token refreshed successfully");
        }

        Ok(tokens.access_token.clone())
    }

    async fn refresh(&self, tokens: &TokenData) -> Result<TokenData> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: u64,
        }

        let resp = self
            .http_client
            .post("https://api.box.com/oauth2/token")
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", tokens.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .context("Failed to contact Box token endpoint")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Token refresh failed ({status}): {body}\n\
                 You may need to re-authenticate with `boxyncd auth`"
            );
        }

        let tr: TokenResponse = resp
            .json()
            .await
            .context("Failed to parse token response")?;
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(tr.expires_in as i64);

        Ok(TokenData {
            access_token: tr.access_token,
            refresh_token: tr.refresh_token,
            expires_at,
        })
    }
}
