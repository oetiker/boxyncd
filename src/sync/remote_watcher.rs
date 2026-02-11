use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::box_api::BoxClient;
use crate::box_api::types::BoxEvent;

use super::state;

/// A single remote event with details needed for targeted sync.
#[derive(Debug, Clone)]
pub struct RemoteEvent {
    pub event_type: String,
    pub box_id: Option<String>,
    pub name: Option<String>,
    pub parent_id: Option<String>,
}

/// A signal from the remote watcher indicating new changes on Box.
#[derive(Debug, Clone)]
pub struct RemoteChange {
    /// Which sync root detected the change (index into config.sync).
    pub root_index: usize,
    /// Individual events for this root (for targeted sync).
    pub events: Vec<RemoteEvent>,
}

/// Sentinel root_index for the single global event stream position.
const GLOBAL_STREAM_INDEX: i64 = -1;

/// Start the remote event watcher.
///
/// A single task long-polls the Box Events API and maps each event to the
/// affected sync root by looking up the event source's Box ID (or its
/// parent's) in the DB and config.  Events outside all sync roots are
/// silently discarded.
pub fn start_remote_watcher(
    client: Arc<BoxClient>,
    pool: SqlitePool,
    box_folder_ids: Vec<String>,
    cancel: CancellationToken,
) -> mpsc::UnboundedReceiver<RemoteChange> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        if let Err(e) = long_poll_loop(client, pool, &box_folder_ids, tx, cancel).await {
            tracing::error!(error = %e, "remote watcher terminated with error");
        }
    });

    rx
}

/// The long-poll loop for the global event stream.
///
/// Flow:
/// 1. Get or initialize the stream position from DB
/// 2. Get the long-poll server URL (OPTIONS /events)
/// 3. Long-poll (blocks until changes or timeout)
/// 4. On change: fetch events, map to roots, notify channel
/// 5. Repeat (re-acquire long-poll URL periodically as it expires)
async fn long_poll_loop(
    client: Arc<BoxClient>,
    pool: SqlitePool,
    box_folder_ids: &[String],
    tx: mpsc::UnboundedSender<RemoteChange>,
    cancel: CancellationToken,
) -> Result<()> {
    // Initialize global stream position, migrating from per-root if needed.
    let mut stream_pos = match state::get_stream_position(&pool, GLOBAL_STREAM_INDEX).await? {
        Some(pos) => {
            tracing::debug!(position = %pos, "resuming from saved stream position");
            pos
        }
        None => {
            // Check for legacy per-root positions and pick the earliest
            let mut earliest: Option<String> = None;
            for i in 0..box_folder_ids.len() {
                if let Some(pos) = state::get_stream_position(&pool, i as i64).await? {
                    if earliest.as_ref().is_none_or(|e| pos < *e) {
                        earliest = Some(pos);
                    }
                }
            }

            let pos = match earliest {
                Some(pos) => {
                    tracing::info!(position = %pos, "migrated from per-root stream positions");
                    pos
                }
                None => {
                    let pos = client.get_stream_position().await?;
                    tracing::info!(position = %pos, "initialized stream position");
                    pos
                }
            };

            state::set_stream_position(&pool, GLOBAL_STREAM_INDEX, &pos).await?;
            pos
        }
    };

    loop {
        if cancel.is_cancelled() {
            tracing::debug!("remote watcher cancelled");
            return Ok(());
        }

        // Step 1: Get long-poll server configuration
        let server = match client.get_long_poll_info().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "failed to get long-poll info, retrying in 30s");
                tokio::select! {
                    _ = cancel.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(30)) => continue,
                }
            }
        };

        // Step 2: Determine how many retries we can do on this server URL
        let max_retries: u32 = server
            .max_retries
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        for _retry in 0..max_retries {
            if cancel.is_cancelled() {
                return Ok(());
            }

            // Step 3: Long-poll (blocks for up to ~10 minutes)
            tracing::debug!("long-polling for remote changes");
            let has_changes = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                result = client.long_poll(&server) => {
                    match result {
                        Ok(changed) => changed,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "long-poll failed, will re-acquire server URL"
                            );
                            break; // break inner loop to re-acquire server URL
                        }
                    }
                }
            };

            if !has_changes {
                // Timeout / reconnect — no new changes, poll again
                continue;
            }

            // Step 4: Fetch events and map to sync roots
            tracing::debug!("new remote changes detected, fetching events");
            match client.get_sync_events(&stream_pos).await {
                Ok((events, next_pos)) => {
                    // Update stream position
                    stream_pos = next_pos;
                    if let Err(e) =
                        state::set_stream_position(&pool, GLOBAL_STREAM_INDEX, &stream_pos).await
                    {
                        tracing::error!(error = %e, "failed to save stream position");
                    }

                    // Map events to affected roots, grouping by root_index
                    let mut root_events: std::collections::HashMap<usize, Vec<RemoteEvent>> =
                        std::collections::HashMap::new();
                    for event in &events {
                        if let Some(root_index) =
                            find_root_for_event(event, &pool, box_folder_ids).await
                        {
                            let source = event.source.as_ref();
                            let name = source.and_then(|s| s.name.as_deref()).unwrap_or("?");
                            tracing::info!(
                                root_index,
                                event_type = %event.event_type,
                                name = %name,
                                "remote event"
                            );
                            root_events
                                .entry(root_index)
                                .or_default()
                                .push(RemoteEvent {
                                    event_type: event.event_type.clone(),
                                    box_id: source.and_then(|s| s.id.clone()),
                                    name: source.and_then(|s| s.name.clone()),
                                    parent_id: source
                                        .and_then(|s| s.parent.as_ref())
                                        .map(|p| p.id.clone()),
                                });
                        } else {
                            tracing::debug!(
                                event_type = %event.event_type,
                                source = ?event.source.as_ref().and_then(|s| s.name.as_deref()),
                                "ignoring event outside sync roots"
                            );
                        }
                    }

                    for (root_index, events) in root_events {
                        let _ = tx.send(RemoteChange { root_index, events });
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to fetch events, will retry");
                }
            }
        }

        // After max_retries, loop back to re-acquire the long-poll server URL
        tracing::debug!("re-acquiring long-poll server URL");
    }
}

/// Determine which sync root (if any) an event belongs to.
async fn find_root_for_event(
    event: &BoxEvent,
    pool: &SqlitePool,
    box_folder_ids: &[String],
) -> Option<usize> {
    let source = event.source.as_ref()?;

    // 1. Look up the item's box_id in our DB
    if let Some(box_id) = &source.id {
        if let Ok(Some(root_index)) = state::find_root_by_box_id(pool, box_id).await {
            return Some(root_index as usize);
        }
    }

    // 2. Check parent — direct match against configured root folder IDs,
    //    then fall back to DB lookup
    if let Some(parent) = &source.parent {
        for (i, folder_id) in box_folder_ids.iter().enumerate() {
            if parent.id == *folder_id {
                return Some(i);
            }
        }

        if let Ok(Some(root_index)) = state::find_root_by_box_id(pool, &parent.id).await {
            return Some(root_index as usize);
        }
    }

    None
}
