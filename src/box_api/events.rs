use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Method;

use super::client::BoxClient;
use super::types::{BoxEvent, EventStream, LongPollInfo, LongPollResponse, RealtimeServer};

/// Event types relevant to file synchronization.
pub const SYNC_EVENT_TYPES: &[&str] = &[
    "ITEM_UPLOAD",
    "ITEM_CREATE",
    "ITEM_MODIFY",
    "ITEM_COPY",
    "ITEM_MOVE",
    "ITEM_RENAME",
    "ITEM_TRASH",
    "ITEM_UNDELETE_VIA_TRASH",
];

impl BoxClient {
    /// Get the long-poll server configuration.
    pub async fn get_long_poll_info(&self) -> Result<RealtimeServer> {
        let resp = self
            .api_request(Method::OPTIONS, "/events")
            .send()
            .await
            .context("Failed to get long-poll info")?;

        let info: LongPollInfo = resp
            .json()
            .await
            .context("Failed to parse long-poll info")?;

        info.entries
            .into_iter()
            .next()
            .context("No realtime server in long-poll response")
    }

    /// Wait for changes using the long-poll URL.
    /// Returns `true` if there are new changes, `false` on timeout/reconnect.
    pub async fn long_poll(&self, server: &RealtimeServer) -> Result<bool> {
        let timeout = server.retry_timeout.unwrap_or(610);

        let resp = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout + 30))
            .build()?
            .get(&server.url)
            .send()
            .await
            .context("Long-poll request failed")?;

        let poll_resp: LongPollResponse = resp
            .json()
            .await
            .context("Failed to parse long-poll response")?;

        Ok(poll_resp.message == "new_change")
    }

    /// Get the current stream position (use "now" to get latest without events).
    pub async fn get_stream_position(&self) -> Result<String> {
        let resp = self
            .api_request(Method::GET, "/events")
            .query(&[("stream_position", "now"), ("stream_type", "changes")])
            .send()
            .await
            .context("Failed to get stream position")?;

        let stream: EventStream = resp
            .json()
            .await
            .context("Failed to parse events response")?;

        Ok(stream.next_stream_position)
    }

    /// Fetch events since the given stream position.
    pub async fn get_events(
        &self,
        stream_position: &str,
        limit: u32,
    ) -> Result<(Vec<BoxEvent>, String)> {
        let limit_str = limit.to_string();
        let resp = self
            .api_request(Method::GET, "/events")
            .query(&[
                ("stream_type", "changes"),
                ("stream_position", stream_position),
                ("limit", &limit_str),
            ])
            .send()
            .await
            .context("Failed to get events")?;

        let stream: EventStream = resp
            .json()
            .await
            .context("Failed to parse events response")?;

        Ok((stream.entries, stream.next_stream_position))
    }

    /// Fetch events and filter to only sync-relevant event types.
    pub async fn get_sync_events(&self, stream_position: &str) -> Result<(Vec<BoxEvent>, String)> {
        let (events, next_pos) = self.get_events(stream_position, 500).await?;

        let filtered: Vec<BoxEvent> = events
            .into_iter()
            .filter(|e| SYNC_EVENT_TYPES.contains(&e.event_type.as_str()))
            .collect();

        Ok((filtered, next_pos))
    }
}
