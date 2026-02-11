use anyhow::{Context, Result};
use reqwest::Method;

use super::client::BoxClient;
use super::types::{BoxFolder, BoxItem, CreateFolderRequest, FolderItems, ParentRef};

/// Fields to request for folder item listings.
const ITEM_FIELDS: &str = "type,id,name,etag,size,sha1,modified_at,content_modified_at,parent";

impl BoxClient {
    /// List all items in a folder, handling pagination automatically.
    /// Returns all files and subfolders (not recursive — one level only).
    pub async fn list_folder_items(&self, folder_id: &str) -> Result<Vec<BoxItem>> {
        let mut all_items = Vec::new();
        let mut marker: Option<String> = None;
        let limit = "1000";

        loop {
            let mut params = vec![
                ("fields", ITEM_FIELDS),
                ("limit", limit),
                ("usemarker", "true"),
            ];
            if let Some(ref m) = marker {
                params.push(("marker", m.as_str()));
            }

            let resp = self
                .api_request(Method::GET, &format!("/folders/{folder_id}/items"))
                .query(&params)
                .send()
                .await
                .with_context(|| format!("Failed to list folder {folder_id}"))?;

            let page: FolderItems = resp
                .json()
                .await
                .context("Failed to parse folder items response")?;

            all_items.extend(page.entries);

            match page.next_marker {
                Some(ref m) if !m.is_empty() => marker = Some(m.clone()),
                _ => break,
            }
        }

        Ok(all_items)
    }

    /// Create a new folder.
    pub async fn create_folder(&self, name: &str, parent_id: &str) -> Result<BoxFolder> {
        let body = CreateFolderRequest {
            name: name.to_string(),
            parent: ParentRef {
                id: parent_id.to_string(),
            },
        };

        let resp = self
            .api_request(Method::POST, "/folders")
            .json(&body)
            .send()
            .await
            .with_context(|| format!("Failed to create folder {name} in {parent_id}"))?;

        if resp.status() == reqwest::StatusCode::CONFLICT {
            anyhow::bail!("Folder '{name}' already exists in parent {parent_id}");
        }

        resp.json()
            .await
            .context("Failed to parse create folder response")
    }

    /// Delete a folder. Set `recursive` to true to delete non-empty folders.
    pub async fn delete_folder(
        &self,
        folder_id: &str,
        etag: Option<&str>,
        recursive: bool,
    ) -> Result<()> {
        let path = if recursive {
            format!("/folders/{folder_id}?recursive=true")
        } else {
            format!("/folders/{folder_id}")
        };

        let mut req = self.api_request(Method::DELETE, &path);
        if let Some(etag) = etag {
            req = req.header("if-match", etag);
        }

        let resp = req
            .send()
            .await
            .with_context(|| format!("Failed to delete folder {folder_id}"))?;

        if resp.status() == reqwest::StatusCode::PRECONDITION_FAILED {
            anyhow::bail!("Folder {folder_id} was modified (etag mismatch)");
        }

        Ok(())
    }
}
