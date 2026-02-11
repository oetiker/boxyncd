use std::path::Path;

use anyhow::{Context, Result};
use sha1::{Digest, Sha1};
use tokio::io::AsyncReadExt;

const BUF_SIZE: usize = 64 * 1024;

/// Compute the SHA-1 hash of a file, reading in 64 KB chunks.
/// Returns the hex-encoded hash string (40 chars, lowercase).
pub async fn compute_sha1(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Cannot open {}", path.display()))?;

    let mut hasher = Sha1::new();
    let mut buf = vec![0u8; BUF_SIZE];

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let hash = hasher.finalize();
    Ok(hex_encode(&hash))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn sha1_of_known_content() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello world").unwrap();
        f.flush().unwrap();

        let hash = compute_sha1(f.path()).await.unwrap();
        // SHA-1 of "hello world" is 2aae6c35c94fcfb415dbe95f408b9ce91ee846ed
        assert_eq!(hash, "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");
    }

    #[tokio::test]
    async fn sha1_of_empty_file() {
        let f = NamedTempFile::new().unwrap();
        let hash = compute_sha1(f.path()).await.unwrap();
        // SHA-1 of empty content is da39a3ee5e6b4b0d3255bfef95601890afd80709
        assert_eq!(hash, "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }
}
