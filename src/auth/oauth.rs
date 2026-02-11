use std::collections::HashMap;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use url::Url;

use super::token_store::{self, TokenData};
use crate::config::Config;

const AUTH_URL: &str = "https://account.box.com/api/oauth2/authorize";
const TOKEN_URL: &str = "https://api.box.com/oauth2/token";
const DEFAULT_PORT: u16 = 8080;

pub async fn run_auth_flow(config: &Config) -> Result<()> {
    let port = config.auth.redirect_port.unwrap_or(DEFAULT_PORT);
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    // Generate CSRF state token
    let mut state_bytes = [0u8; 16];
    rand::fill(&mut state_bytes);
    let state: String = state_bytes.iter().map(|b| format!("{b:02x}")).collect();

    // Build authorization URL
    let auth_url = Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", config.auth.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri.as_str()),
            ("state", state.as_str()),
        ],
    )?;

    // Try to start local HTTP server for callback
    let listener = match TcpListener::bind(format!("127.0.0.1:{port}")).await {
        Ok(l) => {
            tracing::debug!(port, "listening for OAuth callback");
            Some(l)
        }
        Err(e) => {
            tracing::warn!(port, "could not bind for OAuth callback: {e}");
            None
        }
    };

    println!("\nOpen this URL in your browser to authorize boxyncd:\n");
    println!("  {auth_url}\n");

    // Try to open browser (fails silently on headless servers)
    let _ = open::that(auth_url.as_str());

    if listener.is_some() {
        println!("Waiting for authorization...");
        println!("If you're on a remote server, paste the redirect URL or code here:");
    } else {
        println!("Paste the redirect URL or authorization code here:");
    }

    let code = match listener {
        Some(ref l) => {
            tokio::select! {
                result = accept_callback(l, &state) => result?,
                result = read_stdin() => parse_code_input(&result?)?,
            }
        }
        None => parse_code_input(&read_stdin().await?)?,
    };

    println!("\nExchanging authorization code for tokens...");

    let tokens = exchange_code(
        &config.auth.client_id,
        &config.auth.client_secret,
        &code,
        &redirect_uri,
    )
    .await?;

    let token_path = token_store::resolve_token_path(config.auth.token_path.as_deref())?;
    token_store::save_tokens(&token_path, &tokens)?;

    println!(
        "Authorization successful! Tokens saved to {}",
        token_path.display()
    );
    Ok(())
}

/// Listen for the OAuth redirect callback on the local HTTP server.
async fn accept_callback(listener: &TcpListener, expected_state: &str) -> Result<String> {
    loop {
        let (mut stream, addr) = listener.accept().await?;
        tracing::debug!(%addr, "incoming connection");

        let mut buf = vec![0u8; 4096];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);

        let first_line = request.lines().next().unwrap_or("");
        let path = first_line.split_whitespace().nth(1).unwrap_or("");

        // Ignore non-callback requests (favicon, etc.)
        if !path.starts_with("/callback") {
            let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes()).await;
            continue;
        }

        let url =
            Url::parse(&format!("http://localhost{path}")).context("malformed callback URL")?;
        let params: HashMap<_, _> = url.query_pairs().collect();

        // Check for error response from Box
        if let Some(error) = params.get("error") {
            let desc = params
                .get("error_description")
                .map(|s| s.as_ref())
                .unwrap_or("no details");
            let body = format!(
                "<html><body><h1>Authorization failed</h1><p>{error}: {desc}</p></body></html>"
            );
            send_html(&mut stream, 200, &body).await;
            anyhow::bail!("Authorization denied: {error} — {desc}");
        }

        let code = match params.get("code") {
            Some(c) => c.to_string(),
            None => continue,
        };

        // Verify CSRF state
        let recv_state = params.get("state").map(|s| s.as_ref()).unwrap_or("");
        if recv_state != expected_state {
            send_html(
                &mut stream,
                400,
                "<html><body><h1>Invalid state</h1></body></html>",
            )
            .await;
            anyhow::bail!("OAuth state mismatch — possible CSRF attack");
        }

        send_html(
            &mut stream,
            200,
            "<html><body>\
             <h1>Authorization successful!</h1>\
             <p>You can close this tab and return to the terminal.</p>\
             </body></html>",
        )
        .await;

        return Ok(code);
    }
}

async fn send_html(stream: &mut (impl AsyncWriteExt + Unpin), status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Unknown",
    };
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n\
         {body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Read a line from stdin (blocking, runs on a Tokio blocking thread).
async fn read_stdin() -> Result<String> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line)
    })
    .await?
}

/// Parse an authorization code from user input (full redirect URL or bare code).
fn parse_code_input(input: &str) -> Result<String> {
    let input = input.trim();
    anyhow::ensure!(!input.is_empty(), "Empty input");

    // Try to parse as a redirect URL containing ?code=...
    if let Ok(url) = Url::parse(input) {
        let params: HashMap<_, _> = url.query_pairs().collect();
        if let Some(code) = params.get("code") {
            return Ok(code.to_string());
        }
    }

    // Otherwise treat the whole string as a bare authorization code
    Ok(input.to_string())
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenData> {
    #[derive(serde::Deserialize)]
    struct Resp {
        access_token: String,
        refresh_token: String,
        expires_in: u64,
    }

    let resp = reqwest::Client::new()
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .context("Failed to contact Box token endpoint")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Token exchange failed ({status}): {body}");
    }

    let r: Resp = resp
        .json()
        .await
        .context("Failed to parse token response")?;
    let expires_at = chrono::Utc::now() + chrono::Duration::seconds(r.expires_in as i64);

    Ok(TokenData {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_at,
    })
}
