use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Method, RequestBuilder, Response, StatusCode};

use crate::auth::TokenManager;

use super::types::BoxApiError;

const API_BASE: &str = "https://api.box.com/2.0";
const UPLOAD_BASE: &str = "https://upload.box.com/api/2.0";

pub struct BoxClient {
    http: reqwest::Client,
    token_manager: Arc<TokenManager>,
}

impl BoxClient {
    pub fn new(token_manager: Arc<TokenManager>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            token_manager,
        }
    }

    /// Build an authenticated request to the Box API (api.box.com).
    pub fn api_request(&self, method: Method, path: &str) -> AuthenticatedRequest<'_> {
        let url = format!("{API_BASE}{path}");
        AuthenticatedRequest {
            client: self,
            builder: self.http.request(method, &url),
        }
    }

    /// Build an authenticated request to the upload endpoint (upload.box.com).
    pub fn upload_request(&self, method: Method, path: &str) -> AuthenticatedRequest<'_> {
        let url = format!("{UPLOAD_BASE}{path}");
        AuthenticatedRequest {
            client: self,
            builder: self.http.request(method, &url),
        }
    }
}

/// Helper that attaches the bearer token and sends with retry logic.
pub struct AuthenticatedRequest<'a> {
    client: &'a BoxClient,
    builder: RequestBuilder,
}

impl<'a> AuthenticatedRequest<'a> {
    pub fn query(mut self, params: &[(&str, &str)]) -> Self {
        self.builder = self.builder.query(params);
        self
    }

    pub fn header(mut self, key: &str, value: &str) -> Self {
        self.builder = self.builder.header(key, value);
        self
    }

    pub fn json<T: serde::Serialize + ?Sized>(mut self, body: &T) -> Self {
        self.builder = self.builder.json(body);
        self
    }

    pub fn body(mut self, body: reqwest::Body) -> Self {
        self.builder = self.builder.body(body);
        self
    }

    pub fn multipart(mut self, form: reqwest::multipart::Form) -> Self {
        self.builder = self.builder.multipart(form);
        self
    }

    /// Send the request with automatic auth and retry on rate-limit (429).
    pub async fn send(self) -> Result<Response> {
        const MAX_RETRIES: u32 = 5;
        let mut builder = self.builder;

        for attempt in 0..=MAX_RETRIES {
            let token = self.client.token_manager.get_access_token().await?;
            // Clone before consuming — succeeds for non-streamed bodies.
            let retry_builder = builder.try_clone();

            let resp = builder
                .bearer_auth(&token)
                .send()
                .await
                .context("HTTP request failed")?;

            match resp.status() {
                s if s.is_success() || s == StatusCode::FOUND => return Ok(resp),

                StatusCode::TOO_MANY_REQUESTS if attempt < MAX_RETRIES => {
                    let retry_after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(2);
                    let wait = Duration::from_secs(retry_after) + jitter();
                    tracing::warn!(
                        retry_after,
                        attempt = attempt + 1,
                        "rate limited, waiting {wait:?}"
                    );
                    tokio::time::sleep(wait).await;

                    match retry_builder {
                        Some(b) => {
                            builder = b;
                            continue;
                        }
                        None => {
                            anyhow::bail!("Rate limited — cannot retry request with streamed body")
                        }
                    }
                }

                StatusCode::TOO_MANY_REQUESTS => {
                    anyhow::bail!("Rate limited — exhausted {MAX_RETRIES} retries");
                }

                StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => return Ok(resp),

                status => {
                    let body = resp.text().await.unwrap_or_default();
                    let api_err: Option<BoxApiError> = serde_json::from_str(&body).ok();
                    if let Some(err) = api_err {
                        anyhow::bail!("{err}");
                    }
                    anyhow::bail!("Box API error ({status}): {body}");
                }
            }
        }

        unreachable!()
    }
}

impl BoxClient {
    /// GET /users/me — returns the authenticated user's display name.
    pub async fn get_current_user(&self) -> Result<String> {
        let resp = self
            .api_request(Method::GET, "/users/me")
            .query(&[("fields", "name")])
            .send()
            .await
            .context("Failed to fetch current user")?;

        #[derive(serde::Deserialize)]
        struct UserMe {
            name: String,
        }

        let user: UserMe = resp
            .json()
            .await
            .context("Failed to parse /users/me response")?;
        Ok(user.name)
    }
}

fn jitter() -> Duration {
    let ms: u64 = rand::random::<u64>() % 1000;
    Duration::from_millis(ms)
}
