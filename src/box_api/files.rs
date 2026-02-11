use std::path::Path;

use anyhow::{Context, Result};
use reqwest::Method;
use tokio::io::AsyncWriteExt;

use super::client::BoxClient;
use super::types::{
    BoxFile, CommitAttributes, CommitRequest, ParentRef, UploadAttributes, UploadPart,
    UploadPartResponse, UploadResponse, UploadSession,
};

/// Threshold for switching to chunked upload (20 MB).
const CHUNKED_UPLOAD_THRESHOLD: u64 = 20 * 1024 * 1024;

impl BoxClient {
    /// Get file metadata.
    pub async fn get_file(&self, file_id: &str) -> Result<BoxFile> {
        let resp = self
            .api_request(
                Method::GET,
                &format!(
                    "/files/{file_id}?fields=id,name,etag,sha1,size,\
                     modified_at,content_modified_at,parent,file_version,item_status"
                ),
            )
            .send()
            .await
            .with_context(|| format!("Failed to get file {file_id}"))?;

        resp.json().await.context("Failed to parse file response")
    }

    /// Download a file to a local path (atomic: writes to .tmp then renames).
    pub async fn download_file(&self, file_id: &str, dest: &Path) -> Result<()> {
        let resp = self
            .api_request(Method::GET, &format!("/files/{file_id}/content"))
            .send()
            .await
            .with_context(|| format!("Failed to download file {file_id}"))?;

        let tmp_path = dest.with_extension("boxyncd.tmp");

        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .with_context(|| format!("Failed to create temp file: {}", tmp_path.display()))?;

        let bytes = resp.bytes().await.context("Failed to read download body")?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        drop(file);

        tokio::fs::rename(&tmp_path, dest)
            .await
            .with_context(|| format!("Failed to rename temp file to {}", dest.display()))?;

        Ok(())
    }

    /// Upload a new file to Box. Automatically chooses direct or chunked upload.
    pub async fn upload_file(
        &self,
        local_path: &Path,
        parent_id: &str,
        remote_name: &str,
        content_modified_at: Option<&str>,
    ) -> Result<BoxFile> {
        let metadata = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("Cannot stat {}", local_path.display()))?;

        if metadata.len() >= CHUNKED_UPLOAD_THRESHOLD {
            return self
                .chunked_upload_new(local_path, parent_id, remote_name, content_modified_at)
                .await;
        }

        let attrs = UploadAttributes {
            name: remote_name.to_string(),
            parent: ParentRef {
                id: parent_id.to_string(),
            },
            content_modified_at: content_modified_at.map(String::from),
        };
        let attrs_json = serde_json::to_string(&attrs)?;

        let file_bytes = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("Failed to read {}", local_path.display()))?;

        let form = reqwest::multipart::Form::new()
            .text("attributes", attrs_json)
            .part(
                "file",
                reqwest::multipart::Part::bytes(file_bytes).file_name(remote_name.to_string()),
            );

        let resp = self
            .upload_request(Method::POST, "/files/content")
            .multipart(form)
            .send()
            .await
            .context("Failed to upload file")?;

        if resp.status() == reqwest::StatusCode::CONFLICT {
            anyhow::bail!(
                "File '{remote_name}' already exists in folder {parent_id}. \
                 Use upload_new_version instead."
            );
        }

        let upload_resp: UploadResponse = resp
            .json()
            .await
            .context("Failed to parse upload response")?;
        upload_resp
            .entries
            .into_iter()
            .next()
            .context("Upload response contained no file entries")
    }

    /// Upload a new version of an existing file.
    pub async fn upload_new_version(
        &self,
        file_id: &str,
        etag: Option<&str>,
        local_path: &Path,
        content_modified_at: Option<&str>,
    ) -> Result<BoxFile> {
        let metadata = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("Cannot stat {}", local_path.display()))?;

        if metadata.len() >= CHUNKED_UPLOAD_THRESHOLD {
            return self
                .chunked_upload_version(file_id, local_path, content_modified_at)
                .await;
        }

        let name = local_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");

        #[derive(serde::Serialize)]
        struct VersionAttributes {
            name: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            content_modified_at: Option<String>,
        }

        let attrs = VersionAttributes {
            name: name.to_string(),
            content_modified_at: content_modified_at.map(String::from),
        };
        let attrs_json = serde_json::to_string(&attrs)?;

        let file_bytes = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("Failed to read {}", local_path.display()))?;

        let form = reqwest::multipart::Form::new()
            .text("attributes", attrs_json)
            .part(
                "file",
                reqwest::multipart::Part::bytes(file_bytes).file_name(name.to_string()),
            );

        let mut req = self.upload_request(Method::POST, &format!("/files/{file_id}/content"));
        if let Some(etag) = etag {
            req = req.header("if-match", etag);
        }
        let resp = req
            .multipart(form)
            .send()
            .await
            .context("Failed to upload new version")?;

        if resp.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            anyhow::bail!("File {file_id} was modified remotely (etag mismatch)");
        }

        let upload_resp: UploadResponse = resp
            .json()
            .await
            .context("Failed to parse upload response")?;
        upload_resp
            .entries
            .into_iter()
            .next()
            .context("Upload response contained no file entries")
    }

    /// Delete a file on Box.
    pub async fn delete_file(&self, file_id: &str, etag: Option<&str>) -> Result<()> {
        let mut req = self.api_request(Method::DELETE, &format!("/files/{file_id}"));
        if let Some(etag) = etag {
            req = req.header("if-match", etag);
        }

        let resp = req
            .send()
            .await
            .with_context(|| format!("Failed to delete file {file_id}"))?;

        if resp.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            anyhow::bail!("File {file_id} was modified remotely (etag mismatch)");
        }

        Ok(())
    }

    // --- Chunked upload ---

    /// Create a chunked upload session for a new file.
    async fn create_upload_session(
        &self,
        folder_id: &str,
        file_name: &str,
        file_size: u64,
    ) -> Result<UploadSession> {
        #[derive(serde::Serialize)]
        struct Req {
            folder_id: String,
            file_size: u64,
            file_name: String,
        }
        let resp = self
            .upload_request(Method::POST, "/files/upload_sessions")
            .json(&Req {
                folder_id: folder_id.to_string(),
                file_size,
                file_name: file_name.to_string(),
            })
            .send()
            .await
            .context("Failed to create upload session")?;

        resp.json()
            .await
            .context("Failed to parse upload session response")
    }

    /// Create a chunked upload session for a new version of an existing file.
    async fn create_version_upload_session(
        &self,
        file_id: &str,
        file_size: u64,
    ) -> Result<UploadSession> {
        #[derive(serde::Serialize)]
        struct Req {
            file_size: u64,
        }
        let resp = self
            .upload_request(Method::POST, &format!("/files/{file_id}/upload_sessions"))
            .json(&Req { file_size })
            .send()
            .await
            .context("Failed to create version upload session")?;

        resp.json()
            .await
            .context("Failed to parse upload session response")
    }

    /// Upload a single part of a chunked upload.
    async fn upload_part(
        &self,
        session_id: &str,
        part_data: Vec<u8>,
        offset: u64,
        total_size: u64,
    ) -> Result<UploadPart> {
        let part_size = part_data.len() as u64;
        let content_range = format!("bytes {}-{}/{}", offset, offset + part_size - 1, total_size);

        // Compute SHA-1 digest of the part
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(&part_data);
        let sha1_bytes = hasher.finalize();
        let sha1_b64 = base64_encode(&sha1_bytes);
        let digest_header = format!("sha={sha1_b64}");

        let resp = self
            .upload_request(Method::PUT, &format!("/files/upload_sessions/{session_id}"))
            .header("content-range", &content_range)
            .header("digest", &digest_header)
            .header("content-type", "application/octet-stream")
            .body(reqwest::Body::from(part_data))
            .send()
            .await
            .context("Failed to upload part")?;

        let part_resp: UploadPartResponse =
            resp.json().await.context("Failed to parse part response")?;
        Ok(part_resp.part)
    }

    /// Commit a chunked upload session.
    async fn commit_upload(
        &self,
        session_id: &str,
        parts: Vec<UploadPart>,
        file_sha1: &[u8],
        content_modified_at: Option<&str>,
    ) -> Result<BoxFile> {
        let digest_header = format!("sha={}", base64_encode(file_sha1));

        let commit = CommitRequest {
            parts,
            attributes: CommitAttributes {
                content_modified_at: content_modified_at.map(String::from),
            },
        };

        let resp = self
            .upload_request(
                Method::POST,
                &format!("/files/upload_sessions/{session_id}/commit"),
            )
            .header("digest", &digest_header)
            .json(&commit)
            .send()
            .await
            .context("Failed to commit upload")?;

        // Box may return 202 Accepted — need to retry after delay
        if resp.status() == reqwest::StatusCode::ACCEPTED {
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(2);
            tokio::time::sleep(std::time::Duration::from_secs(retry_after)).await;

            // Retry the commit
            let resp2 = self
                .upload_request(
                    Method::POST,
                    &format!("/files/upload_sessions/{session_id}/commit"),
                )
                .header("digest", &digest_header)
                .json(&CommitRequest {
                    parts: vec![], // Box remembers parts from first attempt
                    attributes: CommitAttributes {
                        content_modified_at: content_modified_at.map(String::from),
                    },
                })
                .send()
                .await
                .context("Failed to commit upload (retry)")?;

            let upload_resp: UploadResponse = resp2
                .json()
                .await
                .context("Failed to parse commit response")?;
            return upload_resp
                .entries
                .into_iter()
                .next()
                .context("Commit response contained no file entries");
        }

        let upload_resp: UploadResponse = resp
            .json()
            .await
            .context("Failed to parse commit response")?;
        upload_resp
            .entries
            .into_iter()
            .next()
            .context("Commit response contained no file entries")
    }

    /// Full chunked upload for a new file.
    async fn chunked_upload_new(
        &self,
        local_path: &Path,
        parent_id: &str,
        remote_name: &str,
        content_modified_at: Option<&str>,
    ) -> Result<BoxFile> {
        let file_data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("Failed to read {}", local_path.display()))?;
        let file_size = file_data.len() as u64;

        let session = self
            .create_upload_session(parent_id, remote_name, file_size)
            .await?;

        let (parts, sha1) = self.upload_all_parts(&session, &file_data).await?;

        self.commit_upload(&session.id, parts, &sha1, content_modified_at)
            .await
    }

    /// Full chunked upload for a new version.
    async fn chunked_upload_version(
        &self,
        file_id: &str,
        local_path: &Path,
        content_modified_at: Option<&str>,
    ) -> Result<BoxFile> {
        let file_data = tokio::fs::read(local_path)
            .await
            .with_context(|| format!("Failed to read {}", local_path.display()))?;
        let file_size = file_data.len() as u64;

        let session = self
            .create_version_upload_session(file_id, file_size)
            .await?;

        let (parts, sha1) = self.upload_all_parts(&session, &file_data).await?;

        self.commit_upload(&session.id, parts, &sha1, content_modified_at)
            .await
    }

    /// Upload all parts for a chunked upload session and return (parts, file_sha1).
    async fn upload_all_parts(
        &self,
        session: &UploadSession,
        file_data: &[u8],
    ) -> Result<(Vec<UploadPart>, Vec<u8>)> {
        let part_size = session.part_size as usize;
        let total_size = file_data.len() as u64;
        let mut parts = Vec::new();

        // Compute whole-file SHA-1
        use sha1::{Digest, Sha1};
        let mut file_hasher = Sha1::new();
        file_hasher.update(file_data);
        let file_sha1 = file_hasher.finalize().to_vec();

        for (i, chunk) in file_data.chunks(part_size).enumerate() {
            let offset = (i * part_size) as u64;
            tracing::debug!(
                session_id = %session.id,
                part = i + 1,
                total_parts = session.total_parts,
                "uploading part"
            );
            let part = self
                .upload_part(&session.id, chunk.to_vec(), offset, total_size)
                .await?;
            parts.push(part);
        }

        Ok((parts, file_sha1))
    }
}

/// Simple base64 encoding (standard, no padding). We avoid pulling in the base64 crate
/// for this one use — use a minimal implementation.
fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        let _ = write!(result, "{}", CHARS[((n >> 18) & 0x3F) as usize] as char);
        let _ = write!(result, "{}", CHARS[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            let _ = write!(result, "{}", CHARS[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            let _ = write!(result, "{}", CHARS[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}
