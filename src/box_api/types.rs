use serde::{Deserialize, Serialize};

/// Minimal user representation.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct UserMini {
    pub id: String,
    pub name: Option<String>,
    pub login: Option<String>,
}

/// Minimal folder representation (returned as parent, path entries, etc.).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct FolderMini {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub etag: Option<String>,
}

/// An item in a folder listing — can be a file, folder, or web_link.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BoxItem {
    #[serde(rename = "type")]
    pub item_type: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub modified_at: Option<String>,
    #[serde(default)]
    pub content_modified_at: Option<String>,
    #[serde(default)]
    pub parent: Option<FolderMini>,
}

impl BoxItem {
    pub fn is_file(&self) -> bool {
        self.item_type == "file"
    }
    pub fn is_folder(&self) -> bool {
        self.item_type == "folder"
    }
}

/// Full file metadata from GET /files/{id}.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BoxFile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    pub size: u64,
    #[serde(default)]
    pub modified_at: Option<String>,
    #[serde(default)]
    pub content_modified_at: Option<String>,
    #[serde(default)]
    pub parent: Option<FolderMini>,
    #[serde(default)]
    pub file_version: Option<FileVersionMini>,
    #[serde(default)]
    pub item_status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct FileVersionMini {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: Option<String>,
    pub sha1: Option<String>,
}

/// Full folder metadata from GET /folders/{id}.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BoxFolder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub parent: Option<FolderMini>,
    #[serde(default)]
    pub item_status: Option<String>,
}

/// Paginated response from GET /folders/{id}/items.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct FolderItems {
    pub total_count: Option<u64>,
    pub entries: Vec<BoxItem>,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub next_marker: Option<String>,
}

/// Response from POST /files/content and POST /files/{id}/content.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct UploadResponse {
    pub total_count: u64,
    pub entries: Vec<BoxFile>,
}

/// Attributes JSON for file upload.
#[derive(Debug, Serialize)]
pub struct UploadAttributes {
    pub name: String,
    pub parent: ParentRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_modified_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ParentRef {
    pub id: String,
}

/// Chunked upload session from POST /files/upload_sessions.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct UploadSession {
    pub id: String,
    pub part_size: u64,
    pub total_parts: u32,
    #[serde(default)]
    pub num_parts_processed: u32,
    #[serde(default)]
    pub session_endpoints: Option<SessionEndpoints>,
    #[serde(default)]
    pub session_expires_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct SessionEndpoints {
    pub upload_part: Option<String>,
    pub commit: Option<String>,
    pub abort: Option<String>,
    pub list_parts: Option<String>,
    pub status: Option<String>,
}

/// A single uploaded part (returned by upload_part, needed for commit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadPart {
    pub part_id: String,
    pub offset: u64,
    pub size: u64,
    #[serde(default)]
    pub sha1: Option<String>,
}

/// Response from uploading a single part.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadPartResponse {
    pub part: UploadPart,
}

/// Request body for committing a chunked upload.
#[derive(Debug, Serialize)]
pub struct CommitRequest {
    pub parts: Vec<UploadPart>,
    pub attributes: CommitAttributes,
}

#[derive(Debug, Serialize)]
pub struct CommitAttributes {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_modified_at: Option<String>,
}

/// Long-poll configuration from OPTIONS /events.
#[derive(Debug, Clone, Deserialize)]
pub struct LongPollInfo {
    pub entries: Vec<RealtimeServer>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct RealtimeServer {
    pub url: String,
    #[serde(default)]
    pub ttl: Option<String>,
    #[serde(default)]
    pub max_retries: Option<String>,
    #[serde(default)]
    pub retry_timeout: Option<u64>,
}

/// Response from long-poll GET (the realtime server URL).
#[derive(Debug, Clone, Deserialize)]
pub struct LongPollResponse {
    pub message: String,
}

/// Event stream response from GET /events.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct EventStream {
    pub chunk_size: u64,
    /// Box returns this as either a JSON string or a JSON number.
    #[serde(deserialize_with = "string_or_number")]
    pub next_stream_position: String,
    pub entries: Vec<BoxEvent>,
}

/// Deserialize a value that may be a JSON string or a JSON number into a String.
fn string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrNumber;

    impl<'de> de::Visitor<'de> for StringOrNumber {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or a number")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }
    }

    deserializer.deserialize_any(StringOrNumber)
}

/// A single event from the event stream.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BoxEvent {
    pub event_id: Option<String>,
    pub event_type: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub source: Option<EventSource>,
}

/// The source object attached to an event (file or folder).
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct EventSource {
    #[serde(rename = "type")]
    pub source_type: Option<String>,
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub parent: Option<FolderMini>,
}

/// Request body for creating a folder.
#[derive(Debug, Serialize)]
pub struct CreateFolderRequest {
    pub name: String,
    pub parent: ParentRef,
}

/// Box API error response.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct BoxApiError {
    #[serde(rename = "type")]
    pub error_type: Option<String>,
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: Option<String>,
    pub request_id: Option<String>,
}

impl std::fmt::Display for BoxApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Box API error {}: {} (code: {})",
            self.status.unwrap_or(0),
            self.message.as_deref().unwrap_or("unknown"),
            self.code.as_deref().unwrap_or("none"),
        )
    }
}
